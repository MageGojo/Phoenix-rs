use serde::Serialize;

#[phoenix::contract(page, page = "ssr")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SsrProps {
    pub title: String,
}
