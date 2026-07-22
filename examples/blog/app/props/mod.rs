use serde::Serialize;

use crate::resources::MemberResource;

#[phoenix::contract(page, page = "members/index")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembersPageProps {
    pub members: Vec<MemberResource>,
    pub generated_by: String,
    pub total: u32,
}

#[phoenix::contract(shared)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedProps {
    pub framework: String,
}
