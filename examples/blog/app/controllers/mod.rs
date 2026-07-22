// Controllers keep async signatures so database calls can be added without changing routes.
#![allow(clippy::unused_async)]

use phoenix::prelude::{IntoResponse, Json, Request, Response, StatusCode};
use serde_json::json;

use crate::requests::registration_validator;

pub struct HealthController;

impl HealthController {
    pub async fn show(request: Request) -> Response {
        Json(json!({
            "status": "healthy",
            "route": request.route_name(),
        }))
        .into_response()
    }
}

pub struct UserController;

impl UserController {
    pub async fn show(request: Request) -> Response {
        let user = request.param("user").unwrap_or("unknown");
        Json(json!({
            "user": user,
            "route": request.route_name(),
        }))
        .into_response()
    }
}

pub struct RegistrationController;

impl RegistrationController {
    pub async fn store(request: Request) -> Response {
        let Ok(payload) = request.json() else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "Invalid JSON body." })),
            )
                .into_response();
        };

        match registration_validator(&payload).validate() {
            Ok(()) => (StatusCode::CREATED, Json(json!({ "created": true }))).into_response(),
            Err(errors) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "errors": errors.fields() })),
            )
                .into_response(),
        }
    }
}

pub struct AdminController;

impl AdminController {
    pub async fn dashboard(_request: Request) -> &'static str {
        "admin dashboard"
    }
}
