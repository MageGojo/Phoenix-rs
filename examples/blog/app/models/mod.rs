use std::sync::OnceLock;

use phoenix::database::{Database, DatabaseError, Model, create, models};

static AUTH_STORE: OnceLock<AuthStore> = OnceLock::new();

/// Install the process-wide auth store used by routes and controllers.
///
/// Route files are mounted by `mount_routes!()` without arguments, so the
/// store is installed once at application startup and shared afterwards.
pub fn install_auth_store(store: AuthStore) {
    let _ = AUTH_STORE.set(store);
}

/// The process-wide auth store, when [`install_auth_store`] has run.
#[must_use]
pub fn auth_store() -> Option<AuthStore> {
    AUTH_STORE.get().cloned()
}

#[derive(Clone, Debug, Model)]
pub struct User {
    #[key]
    #[auto]
    pub id: u64,
    #[unique]
    pub email: String,
    pub name: String,
    pub password_hash: String,
    pub role: String,
    pub locked: bool,
}

/// Persistent authentication store backed by the `users` table.
#[derive(Clone)]
pub struct AuthStore {
    database: Database,
}

impl AuthStore {
    /// Build a store with the schema applied (idempotent).
    ///
    /// # Errors
    ///
    /// Returns a database error when the connection or schema setup fails.
    pub async fn new(database: Database) -> Result<Self, DatabaseError> {
        database.initialize_schema().await?;
        Ok(Self { database })
    }

    /// Install this store as the process-wide store used by routes.
    pub fn install(self) {
        install_auth_store(self);
    }

    /// In-memory `SQLite` store for isolated tests.
    ///
    /// # Errors
    ///
    /// Returns a database error when the in-memory database cannot be created.
    pub async fn in_memory() -> Result<Self, DatabaseError> {
        Self::new(Database::sqlite_memory(models!(User)).await?).await
    }

    /// Borrow a dedicated mutable executor that shares the connection pool.
    fn executor(&self) -> impl phoenix::database::Executor + '_ {
        self.database.toasty().clone()
    }

    /// Seed the demo accounts when the table is empty.
    ///
    /// # Errors
    ///
    /// Returns a database or password hashing error when seeding fails.
    pub async fn seed_demo_users(&self) -> Result<(), AuthStoreError> {
        if !self
            .users()
            .await
            .map_err(AuthStoreError::Database)?
            .is_empty()
        {
            return Ok(());
        }
        for (email, name, role, locked) in [
            ("admin@example.test", "Ada Admin", "owner", false),
            ("reviewer@example.test", "Grace Reviewer", "auditor", false),
            ("operator@example.test", "Lin Operator", "operator", true),
        ] {
            let password_hash = phoenix::crypto::Password::hash("phoenix-password")
                .map_err(AuthStoreError::Password)?;
            create!(User {
                email: email,
                name: name,
                password_hash: password_hash,
                role: role,
                locked: locked,
            })
            .exec(&mut self.executor())
            .await
            .map_err(|error| AuthStoreError::Database(DatabaseError::from(error)))?;
        }
        Ok(())
    }

    /// List every user ordered by id.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn users(&self) -> Result<Vec<User>, DatabaseError> {
        let page = User::all()
            .order_by(User::fields().id().asc())
            .paginate(10_000)
            .exec(&mut self.executor())
            .await?;
        Ok(page.iter().cloned().collect())
    }

    /// Find a user by primary key.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn find_user(&self, id: u64) -> Result<Option<User>, DatabaseError> {
        match User::filter_by_id(id).get(&mut self.executor()).await {
            Ok(user) => Ok(Some(user)),
            Err(error) if error.is_record_not_found() => Ok(None),
            Err(error) => Err(DatabaseError::from(error)),
        }
    }

    /// Verify credentials and return the matching user.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn authenticate(
        &self,
        email: &str,
        password: &str,
    ) -> Result<Option<User>, DatabaseError> {
        let normalized = email.trim().to_ascii_lowercase();
        let user = match User::filter_by_email(normalized)
            .get(&mut self.executor())
            .await
        {
            Ok(user) => Some(user),
            Err(error) if error.is_record_not_found() => None,
            Err(error) => return Err(DatabaseError::from(error)),
        };
        let Some(user) = user else { return Ok(None) };
        if user.locked {
            return Ok(None);
        }
        match phoenix::crypto::Password::verify(password, &user.password_hash) {
            Ok(true) => Ok(Some(user)),
            Ok(false) | Err(_) => Ok(None),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthStoreError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Password(#[from] phoenix::crypto::PasswordError),
}
