//! Storage-agnostic authentication: user lookup, Argon2id credentials, and a
//! small session-backed [`AuthGuard`].
//!
//! These pieces are orthogonal to both the persistence layer (behind
//! [`UserProvider`]) and the session backend (behind [`AuthSession`], with a
//! ready-made implementation for `phoenix_security::Session`). The resulting
//! [`AuthUser`] maps onto the existing [`Principal`] so JWT- and session-based
//! subjects share one authorization model.

use phoenix_crypto::Password;
use phoenix_http::BoxFuture;
use phoenix_security::Session;
use serde_json::Value;
use thiserror::Error;

use crate::Principal;

/// Session key under which the signed-in subject id is persisted.
const AUTH_SESSION_KEY: &str = "auth.user_id";

/// An authenticated user record returned by a [`UserProvider`].
///
/// Deliberately minimal: a stable `id`, the public login `identifier` (for
/// example an email address), a display `name`, the Argon2id `password_hash`,
/// and any authorization `roles`. Applications map their own richer row onto
/// this shape inside their provider.
#[derive(Clone, Debug)]
pub struct AuthUser {
    id: String,
    identifier: String,
    name: String,
    password_hash: String,
    roles: Vec<String>,
}

impl AuthUser {
    /// Build a user from its id, login identifier, and Argon2id hash.
    ///
    /// The display name defaults to the identifier until [`Self::with_name`]
    /// overrides it.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        identifier: impl Into<String>,
        password_hash: impl Into<String>,
    ) -> Self {
        let identifier = identifier.into();
        Self {
            id: id.into(),
            name: identifier.clone(),
            identifier,
            password_hash: password_hash.into(),
            roles: Vec::new(),
        }
    }

    /// Override the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Grant one authorization role, reflected in [`Self::to_principal`].
    #[must_use]
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.roles.push(role.into());
        self
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn password_hash(&self) -> &str {
        &self.password_hash
    }

    pub fn roles(&self) -> impl Iterator<Item = &str> {
        self.roles.iter().map(String::as_str)
    }

    /// Reuse the existing [`Principal`] for authorization, so session and JWT
    /// subjects flow through the same RBAC/ABAC engine. The subject is the
    /// stable [`Self::id`].
    #[must_use]
    pub fn to_principal(&self) -> Principal {
        self.roles
            .iter()
            .fold(Principal::new(self.id.clone()), |principal, role| {
                principal.role(role.clone())
            })
    }
}

/// Hash a plaintext password into a self-describing Argon2id PHC string.
///
/// # Errors
///
/// Returns an error when the password is oversized or hashing fails.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    Password::hash(password).map_err(|error| AuthError::Password(error.to_string()))
}

/// Verify a plaintext password against an Argon2id PHC string.
///
/// Returns `false` for a mismatch or any malformed/oversized input, so callers
/// cannot accidentally treat an error as a successful login.
#[must_use]
pub fn verify_password(password: &str, hash: &str) -> bool {
    Password::verify(password, hash).unwrap_or(false)
}

/// A storage-agnostic source of authenticatable users.
///
/// Methods return a boxed `'static` future so the trait stays object-safe and
/// implementations can move an owned handle (a database pool, a fixture list)
/// into the async body.
pub trait UserProvider: Send + Sync {
    /// Look up a user by their public login identifier (for example, email).
    fn find_by_identifier(
        &self,
        identifier: &str,
    ) -> BoxFuture<Result<Option<AuthUser>, AuthError>>;

    /// Look up a user by their stable primary id.
    fn find_by_id(&self, id: &str) -> BoxFuture<Result<Option<AuthUser>, AuthError>>;
}

/// The minimal session surface [`AuthGuard`] needs to persist an identity.
///
/// Implemented for `phoenix_security::Session`; fakes are trivial to write for
/// tests, keeping the guard independent of any concrete session backend.
pub trait AuthSession: Send + Sync {
    /// The stored subject id, if the session currently carries one.
    fn identity(&self) -> Option<String>;
    /// Persist the subject id for subsequent requests.
    fn set_identity(&self, subject: &str);
    /// Forget the stored subject id.
    fn clear_identity(&self);
    /// Rotate the underlying session id (OWASP session-fixation defense).
    fn rotate(&self);
}

impl AuthSession for Session {
    fn identity(&self) -> Option<String> {
        match self.get(AUTH_SESSION_KEY)? {
            Value::String(value) => Some(value),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }

    fn set_identity(&self, subject: &str) {
        self.put(AUTH_SESSION_KEY, subject.to_owned());
    }

    fn clear_identity(&self) {
        self.remove(AUTH_SESSION_KEY);
    }

    fn rotate(&self) {
        self.regenerate();
    }
}

/// A session-backed sign-in helper over a [`UserProvider`].
///
/// Borrows a provider and a session, so it is cheap to build per request:
///
/// ```ignore
/// let guard = AuthGuard::new(&provider, &session);
/// if let Some(user) = guard.attempt(&email, &password).await? {
///     // signed in; `session` now carries the subject id
/// }
/// ```
pub struct AuthGuard<'a, P: UserProvider + ?Sized, S: AuthSession + ?Sized> {
    provider: &'a P,
    session: &'a S,
}

impl<'a, P: UserProvider + ?Sized, S: AuthSession + ?Sized> AuthGuard<'a, P, S> {
    #[must_use]
    pub fn new(provider: &'a P, session: &'a S) -> Self {
        Self { provider, session }
    }

    /// Verify `identifier` + `password`; on success rotate the session, persist
    /// the subject id, and return the user.
    ///
    /// Returns `Ok(None)` for an unknown identifier or a wrong password —
    /// callers must not distinguish the two, to avoid account enumeration.
    ///
    /// # Errors
    ///
    /// Returns an error only when the provider's backing store fails.
    pub async fn attempt(
        &self,
        identifier: &str,
        password: &str,
    ) -> Result<Option<AuthUser>, AuthError> {
        let Some(user) = self.provider.find_by_identifier(identifier).await? else {
            return Ok(None);
        };
        if verify_password(password, user.password_hash()) {
            self.login(&user);
            Ok(Some(user))
        } else {
            Ok(None)
        }
    }

    /// Persist an already-authenticated user into the session, rotating the id.
    pub fn login(&self, user: &AuthUser) {
        self.session.rotate();
        self.session.set_identity(user.id());
    }

    /// Forget the current identity and rotate the session id.
    pub fn logout(&self) {
        self.session.clear_identity();
        self.session.rotate();
    }

    /// Load the currently signed-in user from the session id.
    ///
    /// # Errors
    ///
    /// Returns an error only when the provider's backing store fails.
    pub async fn user(&self) -> Result<Option<AuthUser>, AuthError> {
        let Some(id) = self.session.identity() else {
            return Ok(None);
        };
        self.provider.find_by_id(&id).await
    }

    /// The stored subject id, if the session carries one.
    #[must_use]
    pub fn id(&self) -> Option<String> {
        self.session.identity()
    }
}

/// Authentication failure categories surfaced by the guard and helpers.
#[derive(Debug, Error)]
pub enum AuthError {
    /// The [`UserProvider`] backing store failed.
    #[error("user provider backend error: {0}")]
    Provider(String),
    /// Password hashing failed.
    #[error("password hashing failed: {0}")]
    Password(String),
}

impl AuthError {
    /// Build a [`AuthError::Provider`] from any backend error message.
    #[must_use]
    pub fn provider(message: impl Into<String>) -> Self {
        Self::Provider(message.into())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct MemoryUsers {
        users: Vec<AuthUser>,
    }

    impl UserProvider for MemoryUsers {
        fn find_by_identifier(
            &self,
            identifier: &str,
        ) -> BoxFuture<Result<Option<AuthUser>, AuthError>> {
            let found = self
                .users
                .iter()
                .find(|user| user.identifier() == identifier)
                .cloned();
            Box::pin(async move { Ok(found) })
        }

        fn find_by_id(&self, id: &str) -> BoxFuture<Result<Option<AuthUser>, AuthError>> {
            let found = self.users.iter().find(|user| user.id() == id).cloned();
            Box::pin(async move { Ok(found) })
        }
    }

    #[derive(Default)]
    struct FakeSession {
        identity: Mutex<Option<String>>,
        rotations: Mutex<u32>,
    }

    impl AuthSession for FakeSession {
        fn identity(&self) -> Option<String> {
            self.identity.lock().unwrap().clone()
        }

        fn set_identity(&self, subject: &str) {
            *self.identity.lock().unwrap() = Some(subject.to_owned());
        }

        fn clear_identity(&self) {
            *self.identity.lock().unwrap() = None;
        }

        fn rotate(&self) {
            *self.rotations.lock().unwrap() += 1;
        }
    }

    fn provider() -> MemoryUsers {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(hash.starts_with("$argon2id$v=19$"));
        MemoryUsers {
            users: vec![
                AuthUser::new("7", "ada@example.test", hash)
                    .with_name("Ada")
                    .with_role("member"),
            ],
        }
    }

    #[tokio::test]
    async fn attempt_signs_in_on_the_correct_password_and_rotates_the_session() {
        let provider = provider();
        let session = FakeSession::default();
        let guard = AuthGuard::new(&provider, &session);

        let user = guard
            .attempt("ada@example.test", "correct horse battery staple")
            .await
            .unwrap()
            .expect("valid credentials sign in");
        assert_eq!(user.id(), "7");
        assert_eq!(user.name(), "Ada");
        assert_eq!(session.identity(), Some("7".to_owned()));
        assert_eq!(*session.rotations.lock().unwrap(), 1);

        // The stored identity resolves back to the same user.
        let current = guard.user().await.unwrap().expect("session carries a user");
        assert_eq!(current.identifier(), "ada@example.test");

        // The reused Principal keeps the subject and roles.
        let principal = user.to_principal();
        assert_eq!(principal.subject(), "7");
        assert_eq!(principal.roles().collect::<Vec<_>>(), ["member"]);
    }

    #[tokio::test]
    async fn attempt_rejects_a_wrong_password_without_touching_the_session() {
        let provider = provider();
        let session = FakeSession::default();
        let guard = AuthGuard::new(&provider, &session);

        assert!(
            guard
                .attempt("ada@example.test", "wrong")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            guard
                .attempt("nobody@example.test", "correct horse battery staple")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(session.identity(), None);
        assert_eq!(*session.rotations.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn logout_forgets_the_identity_and_rotates() {
        let provider = provider();
        let session = FakeSession::default();
        let guard = AuthGuard::new(&provider, &session);

        guard
            .attempt("ada@example.test", "correct horse battery staple")
            .await
            .unwrap()
            .unwrap();
        guard.logout();
        assert_eq!(session.identity(), None);
        assert!(guard.user().await.unwrap().is_none());
        // One rotation on login, one on logout.
        assert_eq!(*session.rotations.lock().unwrap(), 2);
    }
}
