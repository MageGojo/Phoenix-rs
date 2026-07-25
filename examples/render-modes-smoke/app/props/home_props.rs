use serde::Serialize;

#[phoenix::contract(page, page = "home")]
#[derive(Serialize)]
pub struct HomeProps {
    pub title: String,
    pub description: String,
}
