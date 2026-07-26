//! Integration tests for the Toasty-backed [`DbCaptchaStore`].
//!
//! They cover the trait contract end to end against `SQLite`:
//!
//! - insert → take → gone, and duplicate-id rejection;
//! - the property the store exists for: **concurrent takes of one challenge
//!   yield exactly one winner**, so a double-submitted captcha cannot be spent
//!   twice even when two requests race;
//! - expired challenges never verify but are still claimed;
//! - `purge_expired` only reclaims what has actually expired;
//! - restart survival (a fresh store over a reopened file database);
//! - the full `Captcha::issue_stored` → `verify_stored` round trip against a
//!   real database.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phoenix_captcha::{
    Captcha, CaptchaRow, CaptchaStore, CaptchaStoreError, DbCaptchaStore, StoredChallenge,
};
use phoenix_database::{Database, TestDatabase, models};

async fn store() -> DbCaptchaStore {
    let database = TestDatabase::new(models!(CaptchaRow))
        .await
        .expect("test database")
        .into_database();
    DbCaptchaStore::new(database)
}

fn challenge(id: &str, expires_at: SystemTime) -> StoredChallenge {
    StoredChallenge {
        id: id.to_owned(),
        answer_hash: "a".repeat(64),
        expires_at,
    }
}

fn in_secs(seconds: u64) -> SystemTime {
    SystemTime::now() + Duration::from_secs(seconds)
}

fn ago(seconds: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(seconds)
}

#[tokio::test]
async fn insert_take_roundtrip_preserves_hash_and_expiry() {
    let store = store().await;
    let expires_at = UNIX_EPOCH + Duration::from_secs(4_000_000_000);
    store
        .insert(challenge("c-1", expires_at))
        .await
        .expect("insert");

    let taken = store.take("c-1").await.expect("take").expect("present");
    assert_eq!(taken.id, "c-1");
    assert_eq!(taken.answer_hash, "a".repeat(64));
    assert_eq!(taken.expires_at, expires_at);

    assert!(
        store.take("c-1").await.expect("take").is_none(),
        "a taken challenge is gone"
    );
    assert!(
        store.take("never-existed").await.expect("take").is_none(),
        "an unknown id is not an error"
    );
}

#[tokio::test]
async fn duplicate_ids_are_rejected() {
    let store = store().await;
    store
        .insert(challenge("c-1", in_secs(60)))
        .await
        .expect("insert");
    assert_eq!(
        store.insert(challenge("c-1", in_secs(60))).await,
        Err(CaptchaStoreError::DuplicateId("c-1".to_owned()))
    );
}

/// The reason a shared store exists: with the hashed answer in one authority,
/// two racing requests for the same challenge must not both succeed.
#[tokio::test]
async fn concurrent_takes_produce_exactly_one_winner() {
    let store = Arc::new(store().await);
    store
        .insert(challenge("race", in_secs(300)))
        .await
        .expect("insert");

    let racers = (0..8).map(|_| {
        let store = Arc::clone(&store);
        tokio::spawn(async move { store.take("race").await.expect("take") })
    });
    let mut winners = 0;
    for racer in racers {
        if racer.await.expect("join").is_some() {
            winners += 1;
        }
    }

    assert_eq!(winners, 1, "exactly one caller may claim a challenge");
    assert!(store.take("race").await.expect("take").is_none());
}

#[tokio::test]
async fn expired_challenges_are_claimed_but_reported_missing() {
    let store = store().await;
    store
        .insert(challenge("stale", ago(1)))
        .await
        .expect("insert");

    assert!(store.take("stale").await.expect("take").is_none());
    // Claimed anyway: the row must not linger for a second attempt.
    assert_eq!(
        store.purge_expired(SystemTime::now()).await,
        Ok(0),
        "the expired row was already removed by take"
    );
}

#[tokio::test]
async fn purge_expired_only_reclaims_expired_rows() {
    let store = store().await;
    store
        .insert(challenge("old-1", ago(120)))
        .await
        .expect("insert");
    store
        .insert(challenge("old-2", ago(1)))
        .await
        .expect("insert");
    store
        .insert(challenge("live", in_secs(300)))
        .await
        .expect("insert");

    assert_eq!(store.purge_expired(SystemTime::now()).await, Ok(2));
    assert!(
        store.take("live").await.expect("take").is_some(),
        "a live challenge survives the purge"
    );
}

#[tokio::test]
async fn issue_and_verify_round_trip_against_the_database() {
    let store = store().await;
    let captcha = Captcha::default();

    let issued = captcha.issue_stored(&store).await.expect("issue");
    // Each glyph is rendered as its own `<text …>G</text>`, in order.
    let answer: String = issued
        .svg
        .split("</text>")
        .filter_map(|chunk| chunk.rsplit_once('>'))
        .map(|(_, glyph)| glyph.to_owned())
        .collect();
    assert_eq!(answer.chars().count(), 5);

    assert!(
        !captcha
            .verify_stored(&store, &issued.id, "wrong")
            .await
            .expect("verify"),
        "a wrong answer fails"
    );
    assert!(
        !captcha
            .verify_stored(&store, &issued.id, &answer)
            .await
            .expect("verify"),
        "and consumes the challenge, so the right answer no longer helps"
    );

    let second = captcha.issue_stored(&store).await.expect("issue");
    let answer: String = second
        .svg
        .split("</text>")
        .filter_map(|chunk| chunk.rsplit_once('>'))
        .map(|(_, glyph)| glyph.to_owned())
        .collect();
    assert!(
        captcha
            .verify_stored(&store, &second.id, &answer.to_uppercase())
            .await
            .expect("verify"),
        "case-insensitive match against the persisted hash"
    );
}

#[tokio::test]
async fn pending_challenges_survive_a_restart() {
    // A file database so a genuinely fresh connection (a "restart") can reopen
    // the same bytes on disk.
    let path = unique_db_path("phoenix_captcha_restart");
    let url = format!("sqlite:{}", path.display());
    let _ = std::fs::remove_file(&path);

    let expires_at = UNIX_EPOCH + Duration::from_secs(4_000_000_000);
    {
        let database = Database::builder(models!(CaptchaRow))
            .connect(&url)
            .await
            .expect("connect");
        database.initialize_schema().await.expect("schema");
        DbCaptchaStore::new(database)
            .insert(challenge("survivor", expires_at))
            .await
            .expect("insert");
    }

    {
        let database = Database::builder(models!(CaptchaRow))
            .connect(&url)
            .await
            .expect("reconnect");
        let taken = DbCaptchaStore::new(database)
            .take("survivor")
            .await
            .expect("take")
            .expect("present after restart");
        assert_eq!(taken.expires_at, expires_at);
    }

    let _ = std::fs::remove_file(&path);
}

fn unique_db_path(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_nanos())
        .unwrap_or_default();
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "{prefix}_{}_{nanos}_{unique}.sqlite3",
        std::process::id()
    ))
}
