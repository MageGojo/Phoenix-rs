// Controllers keep async signatures so database calls can be added without changing routes.
#![allow(clippy::unused_async)]

use phoenix::prelude::{
    IntoResponse, Island, Json, NodeRenderer, Page, RenderContext, RenderMode, Request, Response,
    StatusCode,
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

    pub async fn ssr(request: Request, renderer: NodeRenderer) -> Response {
        render_server_page(article_page().mode(RenderMode::Ssr), &request, &renderer).await
    }

    pub async fn members(request: Request, renderer: NodeRenderer) -> Response {
        let page = Page::new(
            "members/index",
            json!({
                "members": fake_members(),
                "generatedBy": "Rust",
                "total": 100
            }),
        )
        .ssr()
        .script_src(frontend_entry());
        render_server_page(page, &request, &renderer).await
    }
}

async fn render_server_page(page: Page, request: &Request, renderer: &NodeRenderer) -> Response {
    if Page::is_page_request(request.headers()) {
        return page.respond_to(request, None).into_response();
    }

    let context = RenderContext::new(request.uri().to_string()).locale("zh-CN");
    match renderer.render(page.envelope(), &context).await {
        Ok(result) => page
            .trusted_server_html(result.html)
            .respond_to(request, None)
            .into_response(),
        Err(error) => {
            eprintln!("SSR renderer failed: {error}");
            Response::text("SSR renderer unavailable").with_status(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

fn frontend_entry() -> String {
    let vite_url =
        std::env::var("VITE_DEV_URL").unwrap_or_else(|_| "http://127.0.0.1:5173".to_owned());
    format!("{}/views/entry.tsx", vite_url.trim_end_matches('/'))
}

fn fake_members() -> Vec<serde_json::Value> {
    const SURNAMES: [&str; 10] = ["林", "陈", "许", "顾", "沈", "周", "宋", "梁", "叶", "陆"];
    const GIVEN_NAMES: [&str; 10] = [
        "知遥", "景川", "清和", "予安", "星野", "书宁", "嘉树", "云舒", "明澈", "若衡",
    ];
    const CITIES: [&str; 10] = [
        "上海", "杭州", "深圳", "成都", "北京", "苏州", "南京", "武汉", "厦门", "重庆",
    ];
    const ROLES: [&str; 5] = [
        "后端工程师",
        "前端工程师",
        "产品设计师",
        "数据分析师",
        "内容编辑",
    ];
    const STATUSES: [&str; 3] = ["active", "away", "offline"];

    (0..100)
        .map(|index| {
            let id = index + 1;
            json!({
                "id": id,
                "name": format!("{}{}", SURNAMES[index % 10], GIVEN_NAMES[index / 10]),
                "email": format!("member{id:03}@example.test"),
                "city": CITIES[(index * 3 + index / 10 * 2) % CITIES.len()],
                "role": ROLES[(index * 2 + index / 10) % ROLES.len()],
                "status": STATUSES[(index * 11) % STATUSES.len()],
                "projects": (index * 7) % 18 + 1,
                "joinedOn": format!(
                    "2024-{:02}-{:02}",
                    index % 12 + 1,
                    (index * 5) % 28 + 1
                ),
                "lastActiveMinutes": (index * 37) % 1440
            })
        })
        .collect()
}

fn article_page() -> Page {
    Page::new(
        "articles/show",
        json!({
            "title": "React meets Phoenix",
            "summary": "One controller contract, three rendering modes."
        }),
    )
    .script_src(frontend_entry())
    .island(Island::new(
        "article-like",
        "like-button",
        json!({ "initialLikes": 7 }),
    ))
    .trusted_server_html(
        "<main><article><h1>React meets Phoenix</h1><p>One controller contract, three rendering modes.</p></article><div data-phoenix-island=\"article-like\" data-component=\"like-button\"><button>7 likes</button></div></main>",
    )
}
