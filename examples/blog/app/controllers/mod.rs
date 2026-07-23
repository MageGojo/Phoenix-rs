// Controllers keep async signatures so database calls can be added without changing routes.
#![allow(clippy::unused_async)]

use std::sync::atomic::{AtomicU32, Ordering};

use phoenix::prelude::{
    IntoResponse, Json, NodeRenderer, Page, RenderMode, Request, Response, StatusCode, Validated,
};
use serde_json::json;

use crate::{
    auth,
    middleware::AuthorizedAdmin,
    props::{AdminDashboardProps, MembersPageProps, SharedProps},
    requests::{LoginInput, PasswordResetInput, StoreMemberInput, registration_validator},
    resources::{
        AdminUserResource, AuditEventResource, AuthMessageResource, AuthTokenResource,
        MemberResource, MemberStatus,
    },
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

pub struct AuthController;

impl AuthController {
    pub async fn login(Validated(Json(input)): Validated<Json<LoginInput>>) -> Response {
        match auth::authenticate(&input.email, &input.password) {
            Some(user) => Json(AuthTokenResource {
                token_type: "Bearer".to_owned(),
                subject: user.email.to_owned(),
                role: user.role.to_owned(),
                expires_in_seconds: 900,
            })
            .into_response(),
            None => (
                StatusCode::UNAUTHORIZED,
                Json(AuthMessageResource {
                    message: "Invalid credentials.".to_owned(),
                }),
            )
                .into_response(),
        }
    }

    pub async fn logout(_request: Request) -> Json<AuthMessageResource> {
        Json(AuthMessageResource {
            message: "Signed out.".to_owned(),
        })
    }

    pub async fn request_password_reset(
        Validated(Json(_input)): Validated<Json<PasswordResetInput>>,
    ) -> (StatusCode, Json<AuthMessageResource>) {
        (
            StatusCode::ACCEPTED,
            Json(AuthMessageResource {
                message: "If the account exists, reset instructions will be sent.".to_owned(),
            }),
        )
    }
}

pub struct AdminController;

impl AdminController {
    pub async fn dashboard(request: Request, renderer: NodeRenderer) -> Response {
        if request.extensions().get::<AuthorizedAdmin>().is_none() {
            return Response::text("Unauthorized").with_status(StatusCode::UNAUTHORIZED);
        }

        let page = Page::new(
            "admin/dashboard",
            AdminDashboardProps {
                users: auth::users()
                    .into_iter()
                    .map(|user| AdminUserResource {
                        id: user.id,
                        name: user.name.to_owned(),
                        email: user.email.to_owned(),
                        role: user.role.to_owned(),
                        locked: user.locked,
                    })
                    .collect(),
                audit_events: auth::audit_events()
                    .into_iter()
                    .map(|event| AuditEventResource {
                        id: event.id,
                        actor: event.actor.to_owned(),
                        action: event.action.to_owned(),
                        subject: event.subject.to_owned(),
                        occurred_at: event.occurred_at.to_owned(),
                    })
                    .collect(),
                active_sessions: 2,
                pending_password_resets: 1,
            },
        )
        .shared(SharedProps {
            framework: "Phoenix".to_owned(),
        })
        .mode(RenderMode::Spa);
        page.respond_with_renderer(&request, &renderer).await
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
