use phoenix::prelude::{Routes, typed};

use crate::controllers::NoteController;
use crate::requests::StoreNoteRequest;
use crate::resources::NoteResource;


#[must_use]
pub fn routes() -> Routes {
    let member = "/notes/{note}";
    Routes::new()
        .get("/notes", NoteController::index)
        .name("notes.index")
        .get("/notes/create", NoteController::create)
        .name("notes.create")
        .post("/notes", typed(NoteController::store))
        .name("notes.store")
        .action::<StoreNoteRequest, NoteResource>()
        .get(member, NoteController::show)
        .name("notes.show")
        .get(format!("{member}/edit"), NoteController::edit)
        .name("notes.edit")
        .put(member, NoteController::update)
        .name("notes.update")
        .patch(member, NoteController::update)
        .delete(member, NoteController::destroy)
        .name("notes.destroy")
}
