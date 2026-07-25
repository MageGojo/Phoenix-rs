use std::error::Error;

use phoenix::database::Database;

/// Insert repeatable development or test data.
///
/// # Errors
///
/// Returns the first application or database error raised by a seeder.
pub async fn run(_database: &mut Database) -> Result<(), Box<dyn Error>> {
    Ok(())
}
