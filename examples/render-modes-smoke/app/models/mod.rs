// <phoenix:modules>
pub mod note;
pub use note::Note;
// </phoenix:modules>

// <phoenix:model-registry>
// phoenix:model: Note

#[must_use]
pub fn all() -> phoenix::database::ModelSet {
    phoenix::database::models!(
        Note,
    )
}
// </phoenix:model-registry>
