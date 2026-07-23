//! Authentication domain: persistent users, demo fixtures and shared fixtures.
//!
//! The database-backed pieces live in [`crate::models::AuthStore`]; this module
//! keeps the audit-event fixture used by the admin dashboard.

use serde::Serialize;

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
