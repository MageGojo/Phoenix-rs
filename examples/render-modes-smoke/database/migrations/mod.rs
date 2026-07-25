// <phoenix:migration-registry>
// phoenix:migration: m_1784963419850_create_notes_table
pub mod m_1784963419850_create_notes_table;

#[must_use]
pub fn all() -> Vec<phoenix::database::Migration> {
    vec![
        m_1784963419850_create_notes_table::migration(),
    ]
}
// </phoenix:migration-registry>
