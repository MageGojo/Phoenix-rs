use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoUser {
    pub id: u32,
    pub name: &'static str,
    pub email: &'static str,
    pub role: &'static str,
    pub locked: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoAuditEvent {
    pub id: u32,
    pub actor: &'static str,
    pub action: &'static str,
    pub subject: &'static str,
    pub occurred_at: &'static str,
}

#[must_use]
pub fn users() -> Vec<DemoUser> {
    vec![
        DemoUser {
            id: 1,
            name: "Ada Admin",
            email: "admin@example.test",
            role: "owner",
            locked: false,
        },
        DemoUser {
            id: 2,
            name: "Grace Reviewer",
            email: "reviewer@example.test",
            role: "auditor",
            locked: false,
        },
        DemoUser {
            id: 3,
            name: "Lin Operator",
            email: "operator@example.test",
            role: "operator",
            locked: true,
        },
    ]
}

#[must_use]
pub fn audit_events() -> Vec<DemoAuditEvent> {
    vec![
        DemoAuditEvent {
            id: 1001,
            actor: "Ada Admin",
            action: "auth.login",
            subject: "admin@example.test",
            occurred_at: "2026-07-23T09:30:00Z",
        },
        DemoAuditEvent {
            id: 1002,
            actor: "Grace Reviewer",
            action: "users.review",
            subject: "operator@example.test",
            occurred_at: "2026-07-23T09:45:00Z",
        },
    ]
}

#[must_use]
pub fn authenticate(email: &str, password: &str) -> Option<DemoUser> {
    let normalized = email.trim().to_ascii_lowercase();
    if normalized == "admin@example.test" && password == "phoenix-password" {
        users().into_iter().find(|user| user.email == normalized)
    } else {
        None
    }
}
