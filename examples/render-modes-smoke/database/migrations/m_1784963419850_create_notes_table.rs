use phoenix::database::Migration;

#[must_use]
pub fn migration() -> Migration {
    Migration::new("1784963419850", "create notes table")
        .up(
            "CREATE TABLE notes (\
             id INTEGER PRIMARY KEY, \
             name TEXT NOT NULL)",
        )
        .down("DROP TABLE notes")
}
