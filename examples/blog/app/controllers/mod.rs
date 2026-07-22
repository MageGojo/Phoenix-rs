// Controllers keep async signatures so database calls can be added without changing routes.
#![allow(clippy::unused_async)]

use phoenix::prelude::{
    IntoResponse, Island, Json, Page, RenderMode, Request, Response, StatusCode,
};
use serde_json::json;

use crate::{middleware::AuthorizedAdmin, requests::registration_validator};

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
        let payload = match request.json() {
            Ok(payload) => payload,
            Err(rejection) => {
                return (
                    rejection.status(),
                    Json(json!({ "message": rejection.to_string() })),
                )
                    .into_response();
            }
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
    pub async fn dashboard(request: Request) -> &'static str {
        if request.extensions().get::<AuthorizedAdmin>().is_some() {
            "admin dashboard"
        } else {
            "missing authorization context"
        }
    }
}

pub struct ReactController;

impl ReactController {
    pub async fn islands(request: Request) -> Response {
        article_page().respond_to(&request, None).into_response()
    }

    pub async fn spa(request: Request) -> Response {
        article_page()
            .mode(RenderMode::Spa)
            .respond_to(&request, None)
            .into_response()
    }

    pub async fn ssr(request: Request) -> Response {
        article_page()
            .mode(RenderMode::Ssr)
            .respond_to(&request, None)
            .into_response()
    }
}

fn article_page() -> Page {
    Page::new(
        "articles/show",
        json!({
            "title": "React meets Phoenix",
            "summary": "One controller contract, three rendering modes."
        }),
    )
    .island(Island::new(
        "article-like",
        "like-button",
        json!({ "initialLikes": 7 }),
    ))
    .trusted_server_html(
        "<main><article><h1>React meets Phoenix</h1><p>One controller contract, three rendering modes.</p></article><div data-phoenix-island=\"article-like\" data-component=\"like-button\"><button>7 likes</button></div></main>",
    )
}
