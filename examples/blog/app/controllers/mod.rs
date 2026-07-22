// Controllers keep async signatures so database calls can be added without changing routes.
#![allow(clippy::unused_async)]

use std::sync::atomic::{AtomicU32, Ordering};

use phoenix::prelude::{
    IntoResponse, Json, NodeRenderer, Page, RenderMode, Request, Response, StatusCode, Validated,
};
use serde_json::json;

use crate::{
    middleware::AuthorizedAdmin,
    props::{MembersPageProps, SharedProps},
    requests::{StoreMemberInput, registration_validator},
    resources::{MemberResource, MemberStatus},
};

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

static NEXT_MEMBER_ID: AtomicU32 = AtomicU32::new(101);

pub struct MemberController;

impl MemberController {
    pub async fn store(
        Validated(Json(input)): Validated<Json<StoreMemberInput>>,
    ) -> (StatusCode, Json<MemberResource>) {
        let name = input.name.trim();
        let id = NEXT_MEMBER_ID.fetch_add(1, Ordering::Relaxed);
        (
            StatusCode::CREATED,
            Json(MemberResource {
                id,
                name: name.to_owned(),
                email: format!("rust{id}@example.test"),
                city: "Rust 服务端".to_owned(),
                role: "新成员".to_owned(),
                status: MemberStatus::Active,
                projects: 0,
                joined_on: "2026-07-22".to_owned(),
                last_active_minutes: 0,
                created_by: Some("Rust".to_owned()),
            }),
        )
    }
}

impl ReactController {
    pub async fn islands(request: Request, renderer: NodeRenderer) -> Response {
        article_page()
            .respond_with_renderer(&request, &renderer)
            .await
    }

    pub async fn spa(request: Request) -> Response {
        article_page()
            .mode(RenderMode::Spa)
            .respond_to(&request, None)
            .into_response()
    }

    pub async fn ssr(request: Request, renderer: NodeRenderer) -> Response {
        article_page()
            .mode(RenderMode::Ssr)
            .respond_with_renderer(&request, &renderer)
            .await
    }

    pub async fn members(request: Request, renderer: NodeRenderer) -> Response {
        let members = fake_members();
        let page = Page::new(
            "members/index",
            MembersPageProps {
                members,
                generated_by: "Rust".to_owned(),
                total: 100,
            },
        )
        .shared(SharedProps {
            framework: "Phoenix".to_owned(),
        })
        .islands();
        page.respond_with_renderer(&request, &renderer).await
    }
}

fn fake_members() -> Vec<MemberResource> {
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
    const STATUSES: [MemberStatus; 3] = [
        MemberStatus::Active,
        MemberStatus::Away,
        MemberStatus::Offline,
    ];

    (0..100)
        .map(|index| {
            let id = u32::try_from(index + 1).expect("the fixture contains only 100 members");
            MemberResource {
                id,
                name: format!("{}{}", SURNAMES[index % 10], GIVEN_NAMES[index / 10]),
                email: format!("member{id:03}@example.test"),
                city: CITIES[(index * 3 + index / 10 * 2) % CITIES.len()].to_owned(),
                role: ROLES[(index * 2 + index / 10) % ROLES.len()].to_owned(),
                status: STATUSES[(index * 11) % STATUSES.len()],
                projects: u32::try_from((index * 7) % 18 + 1)
                    .expect("fixture project counts are less than 19"),
                joined_on: format!("2024-{:02}-{:02}", index % 12 + 1, (index * 5) % 28 + 1),
                last_active_minutes: u32::try_from((index * 37) % 1440)
                    .expect("fixture activity minutes are less than 1440"),
                created_by: None,
            }
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
}
