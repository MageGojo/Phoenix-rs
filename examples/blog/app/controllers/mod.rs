// Controllers keep async signatures so database calls can be added without changing routes.
#![allow(clippy::unused_async)]

use std::sync::atomic::{AtomicU32, Ordering};

use phoenix::prelude::{
    FromRequest, IntoResponse, Json, NodeRenderer, Page, RenderMode, Request, Response, Session,
    StatusCode, Validated,
};
use serde_json::json;

use crate::{
    auth,
    middleware::CurrentUser,
    models::AuthStore,
    props::{AdminDashboardProps, AuthUserProps, MembersPageProps, SharedProps},
    requests::{LoginInput, PasswordResetInput, StoreMemberInput, registration_validator},
    resources::{
        AdminUserResource, AuditEventResource, AuthMessageResource, AuthSessionResource,
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
    /// Sign in with email + password and start a session.
    ///
    /// # Panics
    ///
    /// Panics when the session middleware is not mounted (framework misconfiguration).
    pub async fn login(request: Request, store: AuthStore) -> Response {
        let Validated(Json(input)) = match Validated::<Json<LoginInput>>::from_request(&request) {
            Ok(input) => input,
            Err(rejection) => return rejection.into_response(),
        };
        let session = request
            .extensions()
            .get::<Session>()
            .cloned()
            .expect("SessionMiddleware is mounted globally");
        match store.authenticate(&input.email, &input.password).await {
            Ok(Some(user)) => {
                // Rotate the session id on privilege change (OWASP session fixation).
                session.regenerate();
                session.put("user_id", user.id);
                Json(AuthSessionResource {
                    subject: user.email.clone(),
                    name: user.name,
                    role: user.role,
                })
                .into_response()
            }
            Ok(None) => (
                StatusCode::UNAUTHORIZED,
                Json(AuthMessageResource {
                    message: "Invalid credentials.".to_owned(),
                }),
            )
                .into_response(),
            Err(error) => Response::text(format!("login store error: {error}"))
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    pub async fn logout(request: Request) -> Json<AuthMessageResource> {
        if let Some(session) = request.extensions().get::<Session>() {
            session.destroy();
        }
        Json(AuthMessageResource {
            message: "Signed out.".to_owned(),
        })
    }

    pub async fn request_password_reset(
        Validated(Json(_input)): Validated<Json<PasswordResetInput>>,
    ) -> (StatusCode, Json<AuthMessageResource>) {
        // Non-goal for now: tokens are only recorded, never emailed (see docs/AUTH_ADMIN.md).
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
    pub async fn dashboard(request: Request, renderer: NodeRenderer, store: AuthStore) -> Response {
        let Some(current_user) = request.extensions().get::<CurrentUser>().cloned() else {
            return Response::text("Unauthorized").with_status(StatusCode::UNAUTHORIZED);
        };
        let users = match store.users().await {
            Ok(users) => users,
            Err(error) => {
                tracing::error!(%error, "admin dashboard failed to load users");
                return Response::text("Internal Server Error")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

        let page = Page::new(
            "admin/dashboard",
            AdminDashboardProps {
                users: users
                    .into_iter()
                    .map(|user| AdminUserResource {
                        id: u32::try_from(user.id).unwrap_or(u32::MAX),
                        name: user.name,
                        email: user.email,
                        role: user.role,
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
                active_sessions: 1,
                pending_password_resets: 0,
            },
        )
        .shared(shared_props(&request, Some(&current_user)))
        .mode(RenderMode::Spa);
        page.respond_with_renderer(&request, &renderer).await
    }
}

fn shared_props(request: &Request, user: Option<&CurrentUser>) -> SharedProps {
    SharedProps {
        framework: "Phoenix".to_owned(),
        user: user.map(|user| AuthUserProps {
            id: u32::try_from(user.id).unwrap_or(u32::MAX),
            name: user.name.clone(),
            email: user.email.clone(),
            role: user.role.clone(),
        }),
        csrf_token: request
            .extensions()
            .get::<Session>()
            .and_then(Session::csrf_token),
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
        .shared(shared_props(&request, None))
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
