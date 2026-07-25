use serde::Serialize;

#[phoenix::contract(page, page = "islands")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandsProps {
    pub title: String,
}
