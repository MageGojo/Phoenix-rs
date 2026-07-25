use serde::Serialize;

#[phoenix::contract(resource)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteResource {
    pub id: String,
    pub name: String,
}
