//! Default-deny RBAC and ABAC authorization for Phoenix applications.

use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    sync::Arc,
};
#[cfg(feature = "jwt")]
use std::marker::PhantomData;

#[cfg(feature = "jwt")]
use phoenix_crypto::{Jwt, JwtClaims};
use phoenix_http::{
    BoxFuture, FromRequest, HeaderValue, IntoResponse, Middleware, Next, Request, Response,
    StatusCode, header,
};
use serde_json::{Map, Value};
use thiserror::Error;

/// A validated, exact authorization capability such as `posts.update`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Permission(Arc<str>);

impl Permission {
    /// # Errors
    ///
    /// Returns an error unless the permission is a non-empty ASCII capability name.
    pub fn new(value: impl AsRef<str>) -> Result<Self, RbacError> {
        let value = value.as_ref();
        if !valid_name(value) {
            return Err(RbacError::InvalidPermission(value.to_owned()));
        }
        Ok(Self(Arc::from(value)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Permission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A role declaration with exact permissions and optional parent roles.
#[derive(Clone, Debug)]
pub struct Role {
    name: Arc<str>,
    permissions: HashSet<Permission>,
    parents: HashSet<Arc<str>>,
}

impl Role {
    /// # Errors
    ///
    /// Returns an error for an invalid role name.
    pub fn new(name: impl AsRef<str>) -> Result<Self, RbacError> {
        let name = name.as_ref();
        if !valid_name(name) {
            return Err(RbacError::InvalidRole(name.to_owned()));
        }
        Ok(Self {
            name: Arc::from(name),
            permissions: HashSet::new(),
            parents: HashSet::new(),
        })
    }

    /// # Errors
    ///
    /// Returns an error for an invalid permission name.
    pub fn allow(mut self, permission: impl AsRef<str>) -> Result<Self, RbacError> {
        self.permissions.insert(Permission::new(permission)?);
        Ok(self)
    }

    /// # Errors
    ///
    /// Returns an error for an invalid parent-role name.
    pub fn inherits(mut self, parent: impl AsRef<str>) -> Result<Self, RbacError> {
        let parent = parent.as_ref();
        if !valid_name(parent) {
            return Err(RbacError::InvalidRole(parent.to_owned()));
        }
        self.parents.insert(Arc::from(parent));
        Ok(self)
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A subject with assigned roles, direct grants/denials, and ABAC attributes.
#[derive(Clone, Debug)]
pub struct Principal {
    subject: Arc<str>,
    roles: HashSet<Arc<str>>,
    permissions: HashSet<Permission>,
    denied_permissions: HashSet<Permission>,
    attributes: Map<String, Value>,
}

impl Principal {
    #[must_use]
    pub fn new(subject: impl Into<String>) -> Self {
        Self {
            subject: Arc::from(subject.into()),
            roles: HashSet::new(),
            permissions: HashSet::new(),
            denied_permissions: HashSet::new(),
            attributes: Map::new(),
        }
    }

    #[must_use]
    pub fn role(mut self, role: impl Into<String>) -> Self {
        self.roles.insert(Arc::from(role.into()));
        self
    }

    /// # Errors
    ///
    /// Returns an error for an invalid permission name.
    pub fn allow(mut self, permission: impl AsRef<str>) -> Result<Self, RbacError> {
        self.permissions.insert(Permission::new(permission)?);
        Ok(self)
    }

    /// Add an explicit subject-level denial. Explicit denial always wins.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid permission name.
    pub fn deny(mut self, permission: impl AsRef<str>) -> Result<Self, RbacError> {
        self.denied_permissions.insert(Permission::new(permission)?);
        Ok(self)
    }

    #[must_use]
    pub fn attribute(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.attributes.insert(name.into(), value.into());
        self
    }

    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn roles(&self) -> impl Iterator<Item = &str> {
        self.roles.iter().map(AsRef::as_ref)
    }

    #[must_use]
    pub fn attribute_value(&self, name: &str) -> Option<&Value> {
        self.attributes.get(name)
    }
}

/// Immutable role graph with inheritance resolved at startup.
#[derive(Clone, Debug, Default)]
pub struct Rbac {
    expanded: Arc<HashMap<Arc<str>, HashSet<Permission>>>,
}

impl Rbac {
    /// Compile roles, rejecting duplicate names, missing parents, and cycles.
    ///
    /// # Errors
    ///
    /// Returns a deterministic role-graph error.
    pub fn build(roles: impl IntoIterator<Item = Role>) -> Result<Self, RbacError> {
        let mut declarations = HashMap::<Arc<str>, Role>::new();
        for role in roles {
            let name = Arc::clone(&role.name);
            if declarations.insert(Arc::clone(&name), role).is_some() {
                return Err(RbacError::DuplicateRole(name.to_string()));
            }
        }
        let mut expanded = HashMap::new();
        for name in declarations.keys() {
            let mut visiting = HashSet::new();
            resolve_role(name, &declarations, &mut expanded, &mut visiting)?;
        }
        Ok(Self {
            expanded: Arc::new(expanded),
        })
    }

    fn evaluate(&self, principal: &Principal, permission: &Permission) -> AuthorizationDecision {
        if principal.denied_permissions.contains(permission) {
            return AuthorizationDecision::Deny;
        }
        if principal.permissions.contains(permission)
            || principal.roles.iter().any(|role| {
                self.expanded
                    .get(role)
                    .is_some_and(|permissions| permissions.contains(permission))
            })
        {
            AuthorizationDecision::Allow
        } else {
            AuthorizationDecision::Abstain
        }
    }
}

fn resolve_role(
    name: &Arc<str>,
    declarations: &HashMap<Arc<str>, Role>,
    expanded: &mut HashMap<Arc<str>, HashSet<Permission>>,
    visiting: &mut HashSet<Arc<str>>,
) -> Result<HashSet<Permission>, RbacError> {
    if let Some(permissions) = expanded.get(name) {
        return Ok(permissions.clone());
    }
    if !visiting.insert(Arc::clone(name)) {
        return Err(RbacError::InheritanceCycle(name.to_string()));
    }
    let role = declarations
        .get(name)
        .ok_or_else(|| RbacError::UnknownRole(name.to_string()))?;
    let mut permissions = role.permissions.clone();
    for parent in &role.parents {
        if !declarations.contains_key(parent) {
            return Err(RbacError::UnknownRole(parent.to_string()));
        }
        permissions.extend(resolve_role(parent, declarations, expanded, visiting)?);
    }
    visiting.remove(name);
    expanded.insert(Arc::clone(name), permissions.clone());
    Ok(permissions)
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-'))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationDecision {
    Allow,
    Deny,
    Abstain,
}

pub struct AuthorizationRequest<'a, Resource> {
    pub principal: &'a Principal,
    pub permission: &'a Permission,
    pub resource: &'a Resource,
}

pub trait AbacPolicy<Resource>: Send + Sync + 'static {
    fn evaluate(&self, request: &AuthorizationRequest<'_, Resource>) -> AuthorizationDecision;
}

pub struct PolicyFn<F>(F);

#[must_use]
pub fn policy_fn<F>(policy: F) -> PolicyFn<F> {
    PolicyFn(policy)
}

impl<Resource, F> AbacPolicy<Resource> for PolicyFn<F>
where
    F: for<'a> Fn(&AuthorizationRequest<'a, Resource>) -> AuthorizationDecision
        + Send
        + Sync
        + 'static,
{
    fn evaluate(&self, request: &AuthorizationRequest<'_, Resource>) -> AuthorizationDecision {
        (self.0)(request)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditReason {
    ExplicitDeny,
    Allowed,
    NoApplicableRule,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizationAuditEvent {
    pub subject: String,
    pub permission: String,
    pub decision: AuthorizationDecision,
    pub reason: AuditReason,
}

pub trait AuthorizationAudit: Send + Sync + 'static {
    fn record(&self, event: &AuthorizationAuditEvent);
}

#[derive(Default)]
struct NoopAudit;

impl AuthorizationAudit for NoopAudit {
    fn record(&self, _event: &AuthorizationAuditEvent) {}
}

/// Deny-overrides authorizer combining exact RBAC grants and ABAC policies.
pub struct AuthorizationEngine<Resource> {
    rbac: Rbac,
    policies: Vec<Arc<dyn AbacPolicy<Resource>>>,
    audit: Arc<dyn AuthorizationAudit>,
}

impl<Resource> Clone for AuthorizationEngine<Resource> {
    fn clone(&self) -> Self {
        Self {
            rbac: self.rbac.clone(),
            policies: self.policies.clone(),
            audit: Arc::clone(&self.audit),
        }
    }
}

impl<Resource: 'static> AuthorizationEngine<Resource> {
    #[must_use]
    pub fn new(rbac: Rbac) -> Self {
        Self {
            rbac,
            policies: Vec::new(),
            audit: Arc::new(NoopAudit),
        }
    }

    #[must_use]
    pub fn policy(mut self, policy: impl AbacPolicy<Resource>) -> Self {
        self.policies.push(Arc::new(policy));
        self
    }

    #[must_use]
    pub fn audit(mut self, audit: impl AuthorizationAudit) -> Self {
        self.audit = Arc::new(audit);
        self
    }

    /// Apply deny-overrides combining: any deny rejects, otherwise any allow permits.
    ///
    /// # Errors
    ///
    /// Returns a generic denial when no rule allows or any rule denies.
    pub fn authorize(
        &self,
        principal: &Principal,
        permission: &Permission,
        resource: &Resource,
    ) -> Result<(), AuthorizationError> {
        let request = AuthorizationRequest {
            principal,
            permission,
            resource,
        };
        let rbac_decision = self.rbac.evaluate(principal, permission);
        let mut allowed = matches!(rbac_decision, AuthorizationDecision::Allow);
        if matches!(rbac_decision, AuthorizationDecision::Deny) {
            self.record(
                principal,
                permission,
                AuthorizationDecision::Deny,
                AuditReason::ExplicitDeny,
            );
            return Err(AuthorizationError::Denied);
        }
        for policy in &self.policies {
            match policy.evaluate(&request) {
                AuthorizationDecision::Deny => {
                    self.record(
                        principal,
                        permission,
                        AuthorizationDecision::Deny,
                        AuditReason::ExplicitDeny,
                    );
                    return Err(AuthorizationError::Denied);
                }
                AuthorizationDecision::Allow => allowed = true,
                AuthorizationDecision::Abstain => {}
            }
        }
        if allowed {
            self.record(
                principal,
                permission,
                AuthorizationDecision::Allow,
                AuditReason::Allowed,
            );
            Ok(())
        } else {
            self.record(
                principal,
                permission,
                AuthorizationDecision::Deny,
                AuditReason::NoApplicableRule,
            );
            Err(AuthorizationError::Denied)
        }
    }

    fn record(
        &self,
        principal: &Principal,
        permission: &Permission,
        decision: AuthorizationDecision,
        reason: AuditReason,
    ) {
        self.audit.record(&AuthorizationAuditEvent {
            subject: principal.subject().to_owned(),
            permission: permission.to_string(),
            decision,
            reason,
        });
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RbacError {
    #[error("invalid role name `{0}`")]
    InvalidRole(String),
    #[error("invalid permission name `{0}`")]
    InvalidPermission(String),
    #[error("duplicate role declaration `{0}`")]
    DuplicateRole(String),
    #[error("unknown inherited role `{0}`")]
    UnknownRole(String),
    #[error("role inheritance cycle at `{0}`")]
    InheritanceCycle(String),
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthorizationError {
    #[error("authorization denied")]
    Denied,
}

/// Authenticated principal extractor populated by an authentication adapter.
pub struct CurrentPrincipal(Arc<Principal>);

impl CurrentPrincipal {
    #[must_use]
    pub fn new(principal: Principal) -> Self {
        Self(Arc::new(principal))
    }
}

impl Clone for CurrentPrincipal {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl Deref for CurrentPrincipal {
    type Target = Principal;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::fmt::Debug for CurrentPrincipal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CurrentPrincipal")
            .field("subject", &self.subject())
            .finish_non_exhaustive()
    }
}

impl FromRequest for CurrentPrincipal {
    type Rejection = PrincipalRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        request
            .extensions()
            .get::<Self>()
            .cloned()
            .ok_or(PrincipalRejection)
    }
}

#[derive(Clone, Copy, Debug, Error)]
#[error("authentication required")]
pub struct PrincipalRejection;

impl IntoResponse for PrincipalRejection {
    fn into_response(self) -> Response {
        unauthorized()
    }
}

/// Convert verified typed JWT claims into an application principal.
#[cfg(feature = "jwt")]
pub struct PrincipalFromJwt<T, F> {
    mapper: Arc<F>,
    marker: PhantomData<fn() -> T>,
}

#[cfg(feature = "jwt")]
impl<T, F> PrincipalFromJwt<T, F> {
    #[must_use]
    pub fn new(mapper: F) -> Self {
        Self {
            mapper: Arc::new(mapper),
            marker: PhantomData,
        }
    }
}

#[cfg(feature = "jwt")]
impl<T, F> Middleware for PrincipalFromJwt<T, F>
where
    T: Send + Sync + 'static,
    F: Fn(&JwtClaims<T>) -> Principal + Send + Sync + 'static,
{
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        let mapper = Arc::clone(&self.mapper);
        Box::pin(async move {
            let Some(claims) = request.extensions().get::<Jwt<T>>() else {
                return unauthorized();
            };
            let principal = mapper(claims);
            request
                .extensions_mut()
                .insert(CurrentPrincipal::new(principal));
            next.run(request).await
        })
    }
}

/// Route middleware for resource-independent permissions.
#[derive(Clone)]
pub struct RequirePermission {
    authorizer: Arc<AuthorizationEngine<()>>,
    permission: Permission,
}

impl RequirePermission {
    #[must_use]
    pub fn new(authorizer: Arc<AuthorizationEngine<()>>, permission: Permission) -> Self {
        Self {
            authorizer,
            permission,
        }
    }
}

impl std::fmt::Debug for RequirePermission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequirePermission")
            .field("permission", &self.permission)
            .finish_non_exhaustive()
    }
}

impl Middleware for RequirePermission {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        let authorizer = Arc::clone(&self.authorizer);
        let permission = self.permission.clone();
        Box::pin(async move {
            let Some(principal) = request.extensions().get::<CurrentPrincipal>() else {
                return unauthorized();
            };
            if authorizer.authorize(principal, &permission, &()).is_err() {
                return Response::text("Forbidden").with_status(StatusCode::FORBIDDEN);
            }
            next.run(request).await
        })
    }
}

fn unauthorized() -> Response {
    let mut response = Response::text("Unauthorized").with_status(StatusCode::UNAUTHORIZED);
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use phoenix_http::{Method, typed};
    use phoenix_routing::Routes;

    use super::*;

    #[cfg(feature = "jwt")]
    use std::time::Duration;

    #[cfg(feature = "jwt")]
    use phoenix_crypto::{
        JwtAuth, JwtConfig, JwtKey, JwtManager, MemoryTokenStore, StatefulJwtAuth, TokenService,
    };
    #[cfg(feature = "jwt")]
    use serde::{Deserialize, Serialize};

    fn rbac() -> Rbac {
        Rbac::build([
            Role::new("writer")
                .unwrap()
                .allow("posts.read")
                .unwrap()
                .allow("posts.update")
                .unwrap(),
            Role::new("admin")
                .unwrap()
                .inherits("writer")
                .unwrap()
                .allow("posts.delete")
                .unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn resolves_role_inheritance_and_rejects_invalid_graphs() {
        let engine = AuthorizationEngine::new(rbac());
        let admin = Principal::new("user-1").role("admin");
        assert!(
            engine
                .authorize(&admin, &Permission::new("posts.update").unwrap(), &())
                .is_ok()
        );

        let cycle = Rbac::build([
            Role::new("one").unwrap().inherits("two").unwrap(),
            Role::new("two").unwrap().inherits("one").unwrap(),
        ]);
        assert!(matches!(cycle, Err(RbacError::InheritanceCycle(_))));
        assert!(matches!(
            Rbac::build([Role::new("child").unwrap().inherits("missing").unwrap()]),
            Err(RbacError::UnknownRole(_))
        ));
        assert_eq!(
            Rbac::build([Role::new("writer").unwrap(), Role::new("writer").unwrap()]).unwrap_err(),
            RbacError::DuplicateRole("writer".to_owned())
        );
    }

    #[derive(Clone)]
    struct Document {
        owner: String,
        classification: &'static str,
    }

    #[test]
    fn abac_allows_owners_but_explicit_denial_overrides_every_grant() {
        let engine = AuthorizationEngine::new(rbac()).policy(policy_fn(
            |request: &AuthorizationRequest<'_, Document>| {
                if request.resource.classification == "restricted"
                    && request
                        .principal
                        .attribute_value("clearance")
                        .and_then(Value::as_str)
                        != Some("restricted")
                {
                    AuthorizationDecision::Deny
                } else if request.resource.owner == request.principal.subject() {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::Abstain
                }
            },
        ));
        let owner = Principal::new("user-1");
        let document = Document {
            owner: "user-1".to_owned(),
            classification: "public",
        };
        assert!(
            engine
                .authorize(&owner, &Permission::new("posts.update").unwrap(), &document)
                .is_ok()
        );

        let denied = Principal::new("user-1")
            .role("admin")
            .deny("posts.update")
            .unwrap();
        assert_eq!(
            engine.authorize(
                &denied,
                &Permission::new("posts.update").unwrap(),
                &document
            ),
            Err(AuthorizationError::Denied)
        );
    }

    #[derive(Default)]
    struct AuditLog(Mutex<Vec<AuthorizationAuditEvent>>);

    impl AuthorizationAudit for Arc<AuditLog> {
        fn record(&self, event: &AuthorizationAuditEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn default_deny_records_a_generic_audit_decision() {
        let audit = Arc::new(AuditLog::default());
        let engine = AuthorizationEngine::new(rbac()).audit(Arc::clone(&audit));
        let result = engine.authorize(
            &Principal::new("user-2"),
            &Permission::new("posts.delete").unwrap(),
            &(),
        );
        assert_eq!(result, Err(AuthorizationError::Denied));
        let events = audit.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].reason, AuditReason::NoApplicableRule);
    }

    #[cfg(feature = "jwt")]
    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct AccessClaims {
        roles: Vec<String>,
    }

    #[cfg(feature = "jwt")]
    #[tokio::test]
    async fn jwt_principal_and_permission_middleware_distinguish_401_and_403() {
        let jwt = Arc::new(
            JwtManager::new(
                JwtKey::new("active", [3_u8; 32]).unwrap(),
                JwtConfig::default(),
            )
            .unwrap(),
        );
        let authorizer = Arc::new(AuthorizationEngine::new(rbac()));
        let router = Routes::new()
            .get(
                "/admin",
                typed(|principal: CurrentPrincipal| async move { principal.subject().to_owned() }),
            )
            .with_middleware(JwtAuth::<AccessClaims>::new(Arc::clone(&jwt)))
            .with_middleware(PrincipalFromJwt::new(|claims: &JwtClaims<AccessClaims>| {
                claims
                    .custom
                    .roles
                    .iter()
                    .fold(Principal::new(&claims.sub), |principal, role| {
                        principal.role(role)
                    })
            }))
            .with_middleware(RequirePermission::new(
                authorizer,
                Permission::new("posts.delete").unwrap(),
            ))
            .build()
            .unwrap();

        let missing = router
            .handle(Request::new(Method::GET, "/admin".parse().unwrap()))
            .await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let member_token = jwt
            .issue(
                "member-1",
                AccessClaims {
                    roles: vec!["writer".to_owned()],
                },
            )
            .unwrap();
        let mut member = Request::new(Method::GET, "/admin".parse().unwrap());
        member.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {member_token}")).unwrap(),
        );
        assert_eq!(router.handle(member).await.status(), StatusCode::FORBIDDEN);

        let admin_token = jwt
            .issue(
                "admin-1",
                AccessClaims {
                    roles: vec!["admin".to_owned()],
                },
            )
            .unwrap();
        let mut admin = Request::new(Method::GET, "/admin".parse().unwrap());
        admin.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {admin_token}")).unwrap(),
        );
        let response = router.handle(admin).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), "admin-1");
    }

    #[cfg(feature = "jwt")]
    #[tokio::test]
    async fn stateful_jwt_revocation_precedes_principal_authorization() {
        let jwt = JwtManager::new(
            JwtKey::new("active", [4_u8; 32]).unwrap(),
            JwtConfig::new(Duration::from_mins(5)),
        )
        .unwrap();
        let service = Arc::new(
            TokenService::new(
                jwt,
                Arc::new(MemoryTokenStore::new()),
                Duration::from_hours(720),
            )
            .unwrap(),
        );
        let authorizer = Arc::new(AuthorizationEngine::new(rbac()));
        let router = Routes::new()
            .get(
                "/admin",
                typed(|principal: CurrentPrincipal| async move { principal.subject().to_owned() }),
            )
            .with_middleware(StatefulJwtAuth::<AccessClaims, _>::new(Arc::clone(
                &service,
            )))
            .with_middleware(PrincipalFromJwt::new(|claims: &JwtClaims<AccessClaims>| {
                claims
                    .custom
                    .roles
                    .iter()
                    .fold(Principal::new(&claims.sub), |principal, role| {
                        principal.role(role)
                    })
            }))
            .with_middleware(RequirePermission::new(
                authorizer,
                Permission::new("posts.delete").unwrap(),
            ))
            .build()
            .unwrap();
        let pair = service
            .issue(
                "admin-2",
                AccessClaims {
                    roles: vec!["admin".to_owned()],
                },
            )
            .await
            .unwrap();

        let request = || {
            let mut request = Request::new(Method::GET, "/admin".parse().unwrap());
            request.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", pair.access_token)).unwrap(),
            );
            request
        };
        assert_eq!(router.handle(request()).await.status(), StatusCode::OK);
        service.revoke_access(&pair.access_token).await.unwrap();
        assert_eq!(
            router.handle(request()).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
