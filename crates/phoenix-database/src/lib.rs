//! Phoenix's stable integration boundary around the Toasty ORM.
//!
//! Models and queries remain native Toasty types. Phoenix owns connection
//! configuration, backend selection, migrations, and isolated test databases.

use std::ops::{Deref, DerefMut};

use thiserror::Error;
use toasty::{Db, ModelSet};

mod migration;

pub use migration::{Migration, MigrationError, MigrationPlan, MigrationRunner, MigrationStatus};

pub use toasty::{
    Deferred, Embed, Executor, Model, Statement, Transaction, TransactionBuilder, batch, create,
    models, query, sql, stmt, update,
};

/// SQL backend selected for this database handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    SQLite,
    PostgreSQL,
}

/// A Toasty database handle with Phoenix deployment metadata.
#[derive(Clone, Debug)]
pub struct Database {
    inner: Db,
    backend: Backend,
}

impl Database {
    /// Start configuring a database with the application's Toasty models.
    #[must_use]
    pub fn builder(models: ModelSet) -> DatabaseBuilder {
        DatabaseBuilder::new(models)
    }

    /// Create a single-connection, isolated in-memory `SQLite` database.
    ///
    /// The schema is not created until [`Self::initialize_schema`] is called.
    ///
    /// # Errors
    ///
    /// Returns an error when the `SQLite` driver cannot initialize.
    pub async fn sqlite_memory(models: ModelSet) -> Result<Self, DatabaseError> {
        DatabaseBuilder::new(models).sqlite_memory().await
    }

    /// Return the configured database backend.
    #[must_use]
    pub const fn backend(&self) -> Backend {
        self.backend
    }

    /// Create all Toasty model tables in a new, empty database.
    ///
    /// # Errors
    ///
    /// Returns an error when Toasty cannot apply the compiled model schema.
    pub async fn initialize_schema(&self) -> Result<(), DatabaseError> {
        self.inner
            .push_schema()
            .await
            .map_err(DatabaseError::Toasty)
    }

    /// Borrow the native Toasty database handle.
    #[must_use]
    pub const fn toasty(&self) -> &Db {
        &self.inner
    }

    /// Mutably borrow the native Toasty database handle.
    pub const fn toasty_mut(&mut self) -> &mut Db {
        &mut self.inner
    }
}

impl Deref for Database {
    type Target = Db;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Database {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Connection and pool configuration shared by `SQLite` and `PostgreSQL`.
pub struct DatabaseBuilder {
    inner: toasty::db::Builder,
}

impl DatabaseBuilder {
    fn new(models: ModelSet) -> Self {
        let mut inner = Db::builder();
        inner.models(models);
        Self { inner }
    }

    /// Set the maximum number of pooled connections.
    #[must_use]
    pub fn max_connections(mut self, maximum: usize) -> Self {
        self.inner.max_pool_size(maximum);
        self
    }

    /// Set a prefix for every model table managed by Toasty.
    #[must_use]
    pub fn table_prefix(mut self, prefix: &str) -> Self {
        self.inner.table_name_prefix(prefix);
        self
    }

    /// Connect using a `sqlite:` or `postgresql:` URL.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported URLs or failed database connections.
    pub async fn connect(mut self, url: &str) -> Result<Database, DatabaseError> {
        let backend = backend_from_url(url)?;
        let inner = self
            .inner
            .connect(url)
            .await
            .map_err(DatabaseError::Toasty)?;
        Ok(Database { inner, backend })
    }

    /// Build an isolated in-memory `SQLite` database.
    ///
    /// # Errors
    ///
    /// Returns an error when the `SQLite` driver cannot initialize.
    pub async fn sqlite_memory(mut self) -> Result<Database, DatabaseError> {
        let driver = toasty::db::Connect::new("sqlite::memory:").await?;
        let inner = self
            .inner
            .build(driver)
            .await
            .map_err(DatabaseError::Toasty)?;
        Ok(Database {
            inner,
            backend: Backend::SQLite,
        })
    }
}

fn backend_from_url(url: &str) -> Result<Backend, DatabaseError> {
    let scheme = url.split_once(':').map_or(url, |(scheme, _)| scheme);
    match scheme {
        "sqlite" => Ok(Backend::SQLite),
        "postgres" | "postgresql" => Ok(Backend::PostgreSQL),
        _ => Err(DatabaseError::UnsupportedBackend(scheme.to_owned())),
    }
}

/// A fresh `SQLite` database owned by one test.
///
/// Constructing one per test avoids shared files, global cleanup, and test
/// ordering dependencies. Dropping it discards all rows.
#[derive(Debug)]
pub struct TestDatabase {
    database: Database,
}

impl TestDatabase {
    /// Create a fresh in-memory database and initialize the model schema.
    ///
    /// # Errors
    ///
    /// Returns an error when database or schema initialization fails.
    pub async fn new(models: ModelSet) -> Result<Self, DatabaseError> {
        let database = Database::sqlite_memory(models).await?;
        database.initialize_schema().await?;
        Ok(Self { database })
    }

    /// Consume the fixture and return its database handle.
    #[must_use]
    pub fn into_database(self) -> Database {
        self.database
    }
}

impl Deref for TestDatabase {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        &self.database
    }
}

impl DerefMut for TestDatabase {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.database
    }
}

/// Database setup error with stable Phoenix categories.
#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("unsupported database URL scheme `{0}`; expected sqlite or postgresql")]
    UnsupportedBackend(String),
    #[error("database operation failed: {0}")]
    Toasty(#[from] toasty::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Model)]
    struct User {
        #[key]
        #[auto]
        id: u64,
        name: String,
    }

    #[tokio::test]
    async fn each_test_database_has_independent_rows() {
        let mut first = TestDatabase::new(models!(User)).await.unwrap();
        let mut second = TestDatabase::new(models!(User)).await.unwrap();

        User::create()
            .name("Ada")
            .exec(first.toasty_mut())
            .await
            .unwrap();

        assert_eq!(User::all().exec(first.toasty_mut()).await.unwrap().len(), 1);
        assert!(
            User::all()
                .exec(second.toasty_mut())
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn accepts_only_supported_sql_url_schemes() {
        assert_eq!(
            backend_from_url("sqlite::memory:").unwrap(),
            Backend::SQLite
        );
        assert_eq!(
            backend_from_url("postgresql://db.invalid/app").unwrap(),
            Backend::PostgreSQL
        );
        assert!(backend_from_url("mysql://db.invalid/app").is_err());
    }
}
