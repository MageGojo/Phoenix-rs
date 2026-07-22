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
