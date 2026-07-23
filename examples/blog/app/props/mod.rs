use serde::Serialize;

use crate::resources::{AdminUserResource, AuditEventResource, MemberResource};

#[phoenix::contract(page, page = "members/index")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembersPageProps {
    pub members: Vec<MemberResource>,
    pub generated_by: String,
    pub total: u32,
}

#[phoenix::contract(resource)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUserProps {
    pub id: u32,
    pub name: String,
    pub email: String,
    pub role: String,
}

#[phoenix::contract(shared)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedProps {
    pub framework: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<AuthUserProps>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csrf_token: Option<String>,
}

#[phoenix::contract(page, page = "admin/dashboard")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminDashboardProps {
    pub users: Vec<AdminUserResource>,
    pub audit_events: Vec<AuditEventResource>,
    pub active_sessions: u32,
    pub pending_password_resets: u32,
}
