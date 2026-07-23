use serde::Serialize;

#[phoenix::contract(resource)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberStatus {
    Active,
    Away,
    Offline,
}

#[phoenix::contract(resource, name = "Member")]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberResource {
    pub id: u32,
    pub name: String,
    pub email: String,
    pub city: String,
    pub role: String,
    pub status: MemberStatus,
    pub projects: u32,
    pub joined_on: String,
    pub last_active_minutes: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

#[phoenix::contract(resource, name = "AdminUser")]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserResource {
    pub id: u32,
    pub name: String,
    pub email: String,
    pub role: String,
    pub locked: bool,
}

#[phoenix::contract(resource, name = "AuditEvent")]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEventResource {
    pub id: u32,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub occurred_at: String,
}

#[phoenix::contract(resource)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthTokenResource {
    pub token_type: String,
    pub subject: String,
    pub role: String,
    pub expires_in_seconds: u32,
}

#[phoenix::contract(resource)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMessageResource {
    pub message: String,
}
