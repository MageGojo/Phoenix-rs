use serde::Serialize;

#[phoenix::contract(page, page = "spa")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaProps {
    pub title: String,
}
