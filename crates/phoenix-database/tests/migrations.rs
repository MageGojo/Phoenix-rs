use phoenix_database::{Database, Migration, MigrationError, MigrationRunner, Model, models, sql};

#[derive(Debug, Model)]
struct Anchor {
    #[key]
    id: i64,
}

fn migrations() -> Vec<Migration> {
    vec![
        Migration::new("202607220001", "create notes")
            .up("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
            .down("DROP TABLE notes"),
        Migration::new("202607220002", "create tags")
            .up("CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .down("DROP TABLE tags"),
    ]
}

async fn sqlite() -> Database {
    Database::sqlite_memory(models!(Anchor)).await.unwrap()
}

async fn table_exists(database: &mut Database, table: &str) -> bool {
    !sql::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1")
        .bind(table)
        .exec(database.toasty_mut())
        .await
        .unwrap()
        .is_empty()
}

#[tokio::test]
async fn initializes_empty_database_applies_status_and_rolls_back() {
    let mut database = sqlite().await;
    let mut runner = MigrationRunner::new(&mut database, migrations()).unwrap();

    assert!(runner.status().await.unwrap().is_empty());
    assert_eq!(runner.plan().await.unwrap().pending.len(), 2);
    assert_eq!(runner.up().await.unwrap(), 2);
    assert_eq!(runner.up().await.unwrap(), 0);

    let status = runner.status().await.unwrap();
    assert_eq!(status.len(), 2);
    assert_eq!(status[0].batch, 1);
    assert_eq!(status[0].checksum.len(), 64);

    assert_eq!(runner.down(1).await.unwrap(), 1);
    assert_eq!(runner.status().await.unwrap().len(), 1);
    drop(runner);
    assert!(table_exists(&mut database, "notes").await);
    assert!(!table_exists(&mut database, "tags").await);
}

#[tokio::test]
async fn rejects_checksum_drift_before_applying_more_changes() {
    let mut database = sqlite().await;
    MigrationRunner::new(&mut database, migrations())
        .unwrap()
        .up()
        .await
        .unwrap();

    let changed = vec![
        Migration::new("202607220001", "create notes")
            .up("CREATE TABLE notes (id INTEGER PRIMARY KEY, changed TEXT)")
            .down("DROP TABLE notes"),
        migrations().remove(1),
    ];
    let error = MigrationRunner::new(&mut database, changed)
        .unwrap()
        .plan()
        .await
        .unwrap_err();
    assert!(matches!(error, MigrationError::ChecksumMismatch { .. }));
}

#[tokio::test]
async fn failed_sql_rolls_back_the_whole_sqlite_batch() {
    let mut database = sqlite().await;
    let broken = vec![
        Migration::new("202607220001", "create transient")
            .up("CREATE TABLE transient_data (id INTEGER PRIMARY KEY)")
            .down("DROP TABLE transient_data"),
        Migration::new("202607220002", "fail")
            .up("CREATE TABL invalid syntax")
            .down("DROP TABLE never_created"),
    ];
    let mut runner = MigrationRunner::new(&mut database, broken).unwrap();
    assert!(runner.up().await.is_err());
    assert!(runner.status().await.unwrap().is_empty());
    drop(runner);
    assert!(!table_exists(&mut database, "transient_data").await);
}

#[tokio::test]
async fn irreversible_migration_fails_closed() {
    let mut database = sqlite().await;
    let migration = Migration::new("202607220001", "irreversible audit")
        .up("CREATE TABLE audit_log (id INTEGER PRIMARY KEY)")
        .irreversible();
    let mut runner = MigrationRunner::new(&mut database, [migration]).unwrap();
    runner.up().await.unwrap();
    let error = runner.down(1).await.unwrap_err();
    assert!(matches!(error, MigrationError::Irreversible(_)));
    assert_eq!(runner.status().await.unwrap().len(), 1);
}

#[test]
fn validates_unique_ordered_migration_ids() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut database = runtime.block_on(sqlite());
    let duplicate = [
        Migration::new("2", "first").up("SELECT 1"),
        Migration::new("2", "second").up("SELECT 1"),
    ];
    assert!(matches!(
        MigrationRunner::new(&mut database, duplicate),
        Err(MigrationError::DuplicateId(_))
    ));
}
