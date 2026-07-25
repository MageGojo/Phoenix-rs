use serde::Serialize;

use crate::resources::NoteResource;

#[phoenix::contract(page, page = "notes/index")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesIndexProps {
    pub title: String,
    pub notes: Vec<NoteResource>,
}
