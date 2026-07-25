use std::sync::Arc;

use phoenix::prelude::{
    JwtAuth, JwtClaims, Principal, PrincipalFromJwt, RequirePermission, RouteGroup, Routes, typed,
};

use crate::controllers::features_controller::{FeaturesController, SmokeClaims};
use crate::features::FeatureServices;

#[must_use]
pub fn http_routes(services: &FeatureServices) -> Routes {
    let authorizer = Arc::clone(&services.authorizer);
    let permission = FeatureServices::admin_permission();
    let jwt = Arc::clone(&services.jwt);

    Routes::new()
        .get("/features/sse", FeaturesController::sse)
        .name("features.sse")
        .get("/features/ws", typed(FeaturesController::ws))
        .name("features.ws")
        .post(
            "/features/password/hash",
            typed(FeaturesController::password_hash),
        )
        .name("features.password.hash")
        .post(
            "/features/password/verify",
            typed(FeaturesController::password_verify),
        )
        .name("features.password.verify")
        .post("/features/jwt/token", typed(FeaturesController::jwt_token))
        .name("features.jwt.token")
        .post("/features/storage", typed(FeaturesController::storage_put))
        .name("features.storage.put")
        .get("/features/storage", typed(FeaturesController::storage_get))
        .name("features.storage.get")
        .post("/features/queue/ping", typed(FeaturesController::queue_ping))
        .name("features.queue.ping")
        .post("/features/mail/send", typed(FeaturesController::mail_send))
        .name("features.mail.send")
        .get("/features/mail/sent", typed(FeaturesController::mail_sent))
        .name("features.mail.sent")
        .group(
            RouteGroup::new().middleware(JwtAuth::<SmokeClaims>::new(Arc::clone(&jwt))),
            |group| {
                group
                    .get("/features/jwt/me", typed(FeaturesController::jwt_me))
                    .name("features.jwt.me")
            },
        )
        .group(
            RouteGroup::new()
                .middleware(JwtAuth::<SmokeClaims>::new(jwt))
                .middleware(PrincipalFromJwt::new(
                    |claims: &JwtClaims<SmokeClaims>| {
                        claims.custom.roles.iter().fold(
                            Principal::new(&claims.sub),
                            |principal, role| principal.role(role),
                        )
                    },
                ))
                .middleware(RequirePermission::new(authorizer, permission)),
            |group| {
                group
                    .get("/features/admin", typed(FeaturesController::admin))
                    .name("features.admin")
            },
        )
}
