use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};
use thiserror::Error;
use toasty::{Connection, Executor, stmt::Value};

use crate::{Backend, Database};

const POSTGRES_LOCK_ID: i64 = 0x0050_484f_454e_4958;
const MYSQL_LOCK_NAME: &str = "phoenix_migrations";
const HEX: &[u8; 16] = b"0123456789abcdef";

/// One ordered, reversible database migration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Migration {
    id: String,
    name: String,
    up: Vec<String>,
    down: Option<Vec<String>>,
}

impl Migration {
    /// Define a migration with a stable, sortable ID and human-readable name.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            up: Vec::new(),
            down: Some(Vec::new()),
        }
    }

    /// Append one backend-compatible statement to the up direction.
    #[must_use]
    pub fn up(mut self, statement: impl Into<String>) -> Self {
        self.up.push(statement.into());
        self
    }

    /// Append one statement to the down direction.
    #[must_use]
    pub fn down(mut self, statement: impl Into<String>) -> Self {
        self.down
            .get_or_insert_with(Vec::new)
            .push(statement.into());
        self
    }

    /// Mark this migration as intentionally irreversible.
    #[must_use]
    pub fn irreversible(mut self) -> Self {
        self.down = None;
        self
    }

    /// Return the migration ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Return the migration name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return a deterministic checksum over identity and both directions.
    #[must_use]
    pub fn checksum(&self) -> String {
        let mut digest = Sha256::new();
        for part in [&self.id, &self.name] {
            digest.update(part.as_bytes());
            digest.update([0]);
        }
        for statement in &self.up {
            digest.update(b"up\0");
            digest.update(statement.as_bytes());
            digest.update([0]);
        }
        match &self.down {
            Some(statements) => {
                for statement in statements {
                    digest.update(b"down\0");
                    digest.update(statement.as_bytes());
                    digest.update([0]);
                }
            }
            None => digest.update(b"irreversible\0"),
        }
        let bytes = digest.finalize();
        let mut checksum = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            checksum.push(char::from(HEX[usize::from(byte >> 4)]));
            checksum.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        checksum
    }
}

/// Persisted migration state read from `phoenix_migrations`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationStatus {
    pub id: String,
    pub name: String,
    pub checksum: String,
    pub batch: i64,
    pub applied_at: String,
}

/// A read-only description of changes the next command would make.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MigrationPlan {
    pub pending: Vec<String>,
    pub applied: Vec<MigrationStatus>,
}

/// Executes a validated migration list against one Phoenix database.
pub struct MigrationRunner<'a> {
    database: &'a mut Database,
    migrations: Vec<Migration>,
}

impl<'a> MigrationRunner<'a> {
    /// Validate and prepare an ordered migration runner.
    ///
    /// # Errors
    ///
    /// Returns an error for empty, duplicate, or out-of-order definitions.
    pub fn new(
        database: &'a mut Database,
        migrations: impl IntoIterator<Item = Migration>,
    ) -> Result<Self, MigrationError> {
        let migrations: Vec<_> = migrations.into_iter().collect();
        validate_migrations(&migrations)?;
        Ok(Self {
            database,
            migrations,
        })
    }

    /// Initialize the migration state table and return applied migrations.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot initialize or read the table.
    pub async fn status(&mut self) -> Result<Vec<MigrationStatus>, MigrationError> {
        let backend = self.database.backend();
        let mut connection = self.database.connection().await?;
        ensure_state_table(&mut connection, backend).await?;
        load_status(&mut connection, backend).await
    }

    /// Return pending and applied migrations without changing application tables.
    ///
    /// # Errors
    ///
    /// Returns an error for database failures, checksum drift, or unknown rows.
    pub async fn plan(&mut self) -> Result<MigrationPlan, MigrationError> {
        let applied = self.status().await?;
        verify_applied(&self.migrations, &applied)?;
        let ids: HashSet<_> = applied.iter().map(|item| item.id.as_str()).collect();
        let pending = self
            .migrations
            .iter()
            .filter(|migration| !ids.contains(migration.id()))
            .map(|migration| migration.id.clone())
            .collect();
        Ok(MigrationPlan { pending, applied })
    }

    /// Apply every pending migration and return the number applied.
    ///
    /// # Errors
    ///
    /// Returns an error when locking, validation, or a migration statement fails.
    pub async fn up(&mut self) -> Result<usize, MigrationError> {
        let backend = self.database.backend();
        let migrations = &self.migrations;
        let mut connection = self.database.connection().await?;
        ensure_state_table(&mut connection, backend).await?;
        match backend {
            Backend::SQLite => up_sqlite(&mut connection, migrations).await,
            Backend::PostgreSQL => up_postgresql(&mut connection, migrations).await,
            Backend::MySQL => up_mysql(&mut connection, migrations).await,
        }
    }

    /// Roll back the most recent `steps` migrations.
    ///
    /// # Errors
    ///
    /// Returns an error for an irreversible migration or database failure.
    pub async fn down(&mut self, steps: usize) -> Result<usize, MigrationError> {
        if steps == 0 {
            return Ok(0);
        }
        let backend = self.database.backend();
        let migrations = &self.migrations;
        let mut connection = self.database.connection().await?;
        ensure_state_table(&mut connection, backend).await?;
        match backend {
            Backend::SQLite => down_sqlite(&mut connection, migrations, steps).await,
            Backend::PostgreSQL => down_postgresql(&mut connection, migrations, steps).await,
            Backend::MySQL => down_mysql(&mut connection, migrations, steps).await,
        }
    }
}

fn validate_migrations(migrations: &[Migration]) -> Result<(), MigrationError> {
    let mut previous = None;
    let mut ids = HashSet::new();
    for migration in migrations {
        if migration.id.trim().is_empty() || migration.name.trim().is_empty() {
            return Err(MigrationError::InvalidDefinition(
                "migration IDs and names must not be empty".to_owned(),
            ));
        }
        if migration.up.is_empty() {
            return Err(MigrationError::InvalidDefinition(format!(
                "migration `{}` has no up statements",
                migration.id
            )));
        }
        if !ids.insert(migration.id.clone()) {
            return Err(MigrationError::DuplicateId(migration.id.clone()));
        }
        if previous.is_some_and(|value: &str| value >= migration.id.as_str()) {
            return Err(MigrationError::InvalidOrder(migration.id.clone()));
        }
        previous = Some(migration.id.as_str());
    }
    Ok(())
}

fn verify_applied(
    migrations: &[Migration],
    applied: &[MigrationStatus],
) -> Result<(), MigrationError> {
    let registered: HashMap<_, _> = migrations
        .iter()
        .map(|migration| (migration.id.as_str(), migration))
        .collect();
    for status in applied {
        let Some(migration) = registered.get(status.id.as_str()) else {
            return Err(MigrationError::UnknownApplied(status.id.clone()));
        };
        let actual = migration.checksum();
        if status.checksum != actual {
            return Err(MigrationError::ChecksumMismatch {
                id: status.id.clone(),
                stored: status.checksum.clone(),
                actual,
            });
        }
    }
    Ok(())
}

async fn ensure_state_table(
    executor: &mut dyn Executor,
    backend: Backend,
) -> Result<(), MigrationError> {
    let sql = match backend {
        Backend::SQLite => {
            "CREATE TABLE IF NOT EXISTS phoenix_migrations (\
             id TEXT PRIMARY KEY, name TEXT NOT NULL, checksum TEXT NOT NULL, \
             batch BIGINT NOT NULL, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)"
        }
        Backend::PostgreSQL => {
            "CREATE TABLE IF NOT EXISTS phoenix_migrations (\
             id TEXT PRIMARY KEY, name TEXT NOT NULL, checksum TEXT NOT NULL, \
             batch BIGINT NOT NULL, applied_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP)"
        }
        Backend::MySQL => {
            "CREATE TABLE IF NOT EXISTS phoenix_migrations (\
             id VARCHAR(255) PRIMARY KEY, name VARCHAR(255) NOT NULL, checksum VARCHAR(64) NOT NULL, \
             batch BIGINT NOT NULL, applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP)"
        }
    };
    toasty::sql::statement(sql).exec(executor).await?;
    Ok(())
}

async fn load_status(
    executor: &mut dyn Executor,
    backend: Backend,
) -> Result<Vec<MigrationStatus>, MigrationError> {
    let applied_at = match backend {
        Backend::SQLite => "CAST(applied_at AS TEXT)",
        Backend::PostgreSQL => "applied_at::text",
        Backend::MySQL => "CAST(applied_at AS CHAR)",
    };
    let rows = toasty::sql::query(format!(
        "SELECT id, name, checksum, batch, {applied_at} \
         FROM phoenix_migrations ORDER BY batch, id"
    ))
    .exec(executor)
    .await?;
    rows.into_iter().map(parse_status).collect()
}

fn parse_status(row: Value) -> Result<MigrationStatus, MigrationError> {
    let Value::Record(record) = row else {
        return Err(MigrationError::InvalidStatusRow);
    };
    let [id, name, checksum, batch, applied_at] = record.fields.as_slice() else {
        return Err(MigrationError::InvalidStatusRow);
    };
    Ok(MigrationStatus {
        id: string_value(id)?,
        name: string_value(name)?,
        checksum: string_value(checksum)?,
        batch: integer_value(batch)?,
        applied_at: string_value(applied_at)?,
    })
}

fn string_value(value: &Value) -> Result<String, MigrationError> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or(MigrationError::InvalidStatusRow)
}

fn integer_value(value: &Value) -> Result<i64, MigrationError> {
    match value {
        Value::I64(value) => Ok(*value),
        Value::U64(value) => i64::try_from(*value).map_err(|_| MigrationError::InvalidStatusRow),
        _ => Err(MigrationError::InvalidStatusRow),
    }
}

fn pending<'a>(
    migrations: &'a [Migration],
    applied: &[MigrationStatus],
) -> Result<(Vec<&'a Migration>, i64), MigrationError> {
    verify_applied(migrations, applied)?;
    let ids: HashSet<_> = applied.iter().map(|status| status.id.as_str()).collect();
    let pending = migrations
        .iter()
        .filter(|migration| !ids.contains(migration.id()))
        .collect();
    let batch = applied
        .iter()
        .map(|status| status.batch)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    Ok((pending, batch))
}

async fn up_sqlite(
    connection: &mut Connection,
    migrations: &[Migration],
) -> Result<usize, MigrationError> {
    toasty::sql::statement("BEGIN IMMEDIATE")
        .exec(connection)
        .await?;
    let result = async {
        let applied = load_status(connection, Backend::SQLite).await?;
        let (pending, batch) = pending(migrations, &applied)?;
        for migration in &pending {
            apply_up(connection, Backend::SQLite, migration, batch).await?;
        }
        Ok::<_, MigrationError>(pending.len())
    }
    .await;
    finish_sqlite_transaction(connection, result).await
}

async fn down_sqlite(
    connection: &mut Connection,
    migrations: &[Migration],
    steps: usize,
) -> Result<usize, MigrationError> {
    toasty::sql::statement("BEGIN IMMEDIATE")
        .exec(connection)
        .await?;
    let result = async {
        let applied = load_status(connection, Backend::SQLite).await?;
        let selected = rollback_selection(migrations, &applied, steps)?;
        for migration in &selected {
            apply_down(connection, Backend::SQLite, migration).await?;
        }
        Ok::<_, MigrationError>(selected.len())
    }
    .await;
    finish_sqlite_transaction(connection, result).await
}

async fn finish_sqlite_transaction(
    connection: &mut Connection,
    result: Result<usize, MigrationError>,
) -> Result<usize, MigrationError> {
    match result {
        Ok(count) => {
            toasty::sql::statement("COMMIT").exec(connection).await?;
            Ok(count)
        }
        Err(error) => {
            let _ = toasty::sql::statement("ROLLBACK").exec(connection).await;
            Err(error)
        }
    }
}

async fn up_postgresql(
    connection: &mut Connection,
    migrations: &[Migration],
) -> Result<usize, MigrationError> {
    acquire_postgres_lock(connection).await?;
    let result = async {
        let applied = load_status(connection, Backend::PostgreSQL).await?;
        let (pending, batch) = pending(migrations, &applied)?;
        for migration in &pending {
            let mut transaction = connection.transaction().await?;
            apply_up(&mut transaction, Backend::PostgreSQL, migration, batch).await?;
            transaction.commit().await?;
        }
        Ok::<_, MigrationError>(pending.len())
    }
    .await;
    release_postgres_lock(connection).await;
    result
}

async fn down_postgresql(
    connection: &mut Connection,
    migrations: &[Migration],
    steps: usize,
) -> Result<usize, MigrationError> {
    acquire_postgres_lock(connection).await?;
    let result = async {
        let applied = load_status(connection, Backend::PostgreSQL).await?;
        let selected = rollback_selection(migrations, &applied, steps)?;
        for migration in &selected {
            let mut transaction = connection.transaction().await?;
            apply_down(&mut transaction, Backend::PostgreSQL, migration).await?;
            transaction.commit().await?;
        }
        Ok::<_, MigrationError>(selected.len())
    }
    .await;
    release_postgres_lock(connection).await;
    result
}

async fn acquire_postgres_lock(connection: &mut Connection) -> Result<(), MigrationError> {
    let rows = toasty::sql::query("SELECT pg_try_advisory_lock($1)")
        .bind(POSTGRES_LOCK_ID)
        .exec(connection)
        .await?;
    let acquired = rows
        .first()
        .and_then(Value::as_record)
        .and_then(|record| record.first())
        .is_some_and(|value| matches!(value, Value::Bool(true)));
    if acquired {
        Ok(())
    } else {
        Err(MigrationError::LockUnavailable)
    }
}

async fn release_postgres_lock(connection: &mut Connection) {
    let _ = toasty::sql::query("SELECT pg_advisory_unlock($1)")
        .bind(POSTGRES_LOCK_ID)
        .exec(connection)
        .await;
}

async fn up_mysql(
    connection: &mut Connection,
    migrations: &[Migration],
) -> Result<usize, MigrationError> {
    acquire_mysql_lock(connection).await?;
    let result = async {
        let applied = load_status(connection, Backend::MySQL).await?;
        let (pending, batch) = pending(migrations, &applied)?;
        for migration in &pending {
            let mut transaction = connection.transaction().await?;
            apply_up(&mut transaction, Backend::MySQL, migration, batch).await?;
            transaction.commit().await?;
        }
        Ok::<_, MigrationError>(pending.len())
    }
    .await;
    release_mysql_lock(connection).await;
    result
}

async fn down_mysql(
    connection: &mut Connection,
    migrations: &[Migration],
    steps: usize,
) -> Result<usize, MigrationError> {
    acquire_mysql_lock(connection).await?;
    let result = async {
        let applied = load_status(connection, Backend::MySQL).await?;
        let selected = rollback_selection(migrations, &applied, steps)?;
        for migration in &selected {
            let mut transaction = connection.transaction().await?;
            apply_down(&mut transaction, Backend::MySQL, migration).await?;
            transaction.commit().await?;
        }
        Ok::<_, MigrationError>(selected.len())
    }
    .await;
    release_mysql_lock(connection).await;
    result
}

async fn acquire_mysql_lock(connection: &mut Connection) -> Result<(), MigrationError> {
    let rows = toasty::sql::query("SELECT GET_LOCK(?, 0)")
        .bind(MYSQL_LOCK_NAME)
        .exec(connection)
        .await?;
    let acquired = rows
        .first()
        .and_then(Value::as_record)
        .and_then(|record| record.first())
        .is_some_and(|value| matches!(value, Value::I64(1) | Value::U64(1) | Value::Bool(true)));
    if acquired {
        Ok(())
    } else {
        Err(MigrationError::LockUnavailable)
    }
}

async fn release_mysql_lock(connection: &mut Connection) {
    let _ = toasty::sql::query("SELECT RELEASE_LOCK(?)")
        .bind(MYSQL_LOCK_NAME)
        .exec(connection)
        .await;
}

async fn apply_up(
    executor: &mut dyn Executor,
    backend: Backend,
    migration: &Migration,
    batch: i64,
) -> Result<(), MigrationError> {
    for statement in &migration.up {
        toasty::sql::statement(statement).exec(executor).await?;
    }
    let placeholders = placeholders(backend, 4);
    toasty::sql::statement(format!(
        "INSERT INTO phoenix_migrations (id, name, checksum, batch) VALUES ({})",
        placeholders.join(", ")
    ))
    .bind(&migration.id)
    .bind(&migration.name)
    .bind(migration.checksum())
    .bind(batch)
    .exec(executor)
    .await?;
    Ok(())
}

async fn apply_down(
    executor: &mut dyn Executor,
    backend: Backend,
    migration: &Migration,
) -> Result<(), MigrationError> {
    let statements = migration
        .down
        .as_ref()
        .filter(|statements| !statements.is_empty())
        .ok_or_else(|| MigrationError::Irreversible(migration.id.clone()))?;
    for statement in statements.iter().rev() {
        toasty::sql::statement(statement).exec(executor).await?;
    }
    toasty::sql::statement(format!(
        "DELETE FROM phoenix_migrations WHERE id = {}",
        placeholders(backend, 1)[0]
    ))
    .bind(&migration.id)
    .exec(executor)
    .await?;
    Ok(())
}

fn rollback_selection<'a>(
    migrations: &'a [Migration],
    applied: &[MigrationStatus],
    steps: usize,
) -> Result<Vec<&'a Migration>, MigrationError> {
    verify_applied(migrations, applied)?;
    let registered: HashMap<_, _> = migrations
        .iter()
        .map(|migration| (migration.id.as_str(), migration))
        .collect();
    applied
        .iter()
        .rev()
        .take(steps)
        .map(|status| {
            registered
                .get(status.id.as_str())
                .copied()
                .ok_or_else(|| MigrationError::UnknownApplied(status.id.clone()))
        })
        .collect()
}

fn placeholders(backend: Backend, count: usize) -> Vec<String> {
    (1..=count)
        .map(|index| match backend {
            Backend::SQLite => format!("?{index}"),
            Backend::PostgreSQL => format!("${index}"),
            Backend::MySQL => "?".to_owned(),
        })
        .collect()
}

/// Migration validation or execution error.
#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("invalid migration definition: {0}")]
    InvalidDefinition(String),
    #[error("duplicate migration ID `{0}`")]
    DuplicateId(String),
    #[error("migration `{0}` is not in strictly increasing ID order")]
    InvalidOrder(String),
    #[error("database contains unknown applied migration `{0}`")]
    UnknownApplied(String),
    #[error("migration `{id}` checksum changed (stored {stored}, current {actual})")]
    ChecksumMismatch {
        id: String,
        stored: String,
        actual: String,
    },
    #[error("migration `{0}` is irreversible")]
    Irreversible(String),
    #[error("another migration runner holds the database lock")]
    LockUnavailable,
    #[error("the migration status table returned an invalid row")]
    InvalidStatusRow,
    #[error(transparent)]
    Database(#[from] toasty::Error),
}
