//! Integration contracts gated by `PHOENIX_TEST_REDIS_URL`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use phoenix_crypto::{RefreshRecord, RotateRefresh, TokenStore};
use phoenix_security::{RateLimitBackend, RateLimitDecision, SessionBackend, SessionWrite};
use serde_json::json;

use phoenix_redis::RedisStores;

fn redis_url() -> Option<String> {
    std::env::var("PHOENIX_TEST_REDIS_URL")
        .ok()
        .filter(|value| !value.is_empty())
}

async fn stores() -> Option<RedisStores> {
    let url = redis_url()?;
    match RedisStores::connect(&url).await {
        Ok(stores) => Some(stores),
        Err(error) => {
            eprintln!("skipping redis integration: {error}");
            None
        }
    }
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    )
}

#[tokio::test]
async fn dual_clients_session_conflict_rotate_and_delete() {
    let Some(stores) = stores().await else {
        return;
    };
    let first = stores.session();
    let second = stores.session();
    let old = unique("sess-old");
    let new = unique("sess-new");

    assert_eq!(
        first
            .create(
                old.clone(),
                HashMap::from([("n".to_owned(), json!(1))]),
                200
            )
            .await
            .unwrap(),
        SessionWrite::Saved { version: 1 }
    );
    assert_eq!(
        second
            .create(old.clone(), HashMap::new(), 200)
            .await
            .unwrap(),
        SessionWrite::Collision
    );

    let left = first
        .load(old.clone(), 100, 250)
        .await
        .unwrap()
        .expect("session");
    assert_eq!(left.version, 1);
    assert_eq!(left.expires_at, 250);
    let right = second
        .load(old.clone(), 100, 250)
        .await
        .unwrap()
        .expect("session");
    assert_eq!(
        first
            .save(old.clone(), left.version, HashMap::new(), 300)
            .await
            .unwrap(),
        SessionWrite::Saved { version: 2 }
    );
    assert_eq!(
        second
            .save(old.clone(), right.version, HashMap::new(), 300)
            .await
            .unwrap(),
        SessionWrite::Conflict
    );
    assert_eq!(
        second
            .rotate(old.clone(), new.clone(), 2, HashMap::new(), 400)
            .await
            .unwrap(),
        SessionWrite::Saved { version: 3 }
    );
    assert!(first.load(old, 150, 400).await.unwrap().is_none());
    assert_eq!(
        first.delete(new.clone(), 3).await.unwrap(),
        SessionWrite::Saved { version: 4 }
    );
    assert_eq!(second.delete(new, 3).await.unwrap(), SessionWrite::Missing);
}

#[tokio::test]
async fn dual_clients_rate_limit_accumulates() {
    let Some(stores) = stores().await else {
        return;
    };
    let first = stores.rate_limit();
    let second = stores.rate_limit();
    let key = unique("rl");
    let window = Duration::from_mins(1);

    let first_hit = first.hit(key.clone(), 2, window, 1_000).await.unwrap();
    assert_eq!(
        first_hit,
        RateLimitDecision {
            allowed: true,
            remaining: 1,
            retry_after: Duration::from_mins(1),
        }
    );
    let second_hit = second.hit(key.clone(), 2, window, 1_001).await.unwrap();
    assert_eq!(
        second_hit,
        RateLimitDecision {
            allowed: true,
            remaining: 0,
            retry_after: Duration::from_secs(59),
        }
    );
    let third = first.hit(key, 2, window, 1_002).await.unwrap();
    assert!(!third.allowed);
    assert_eq!(third.remaining, 0);
}

#[tokio::test]
async fn dual_clients_refresh_reuse_revokes_family() {
    let Some(stores) = stores().await else {
        return;
    };
    let first: Arc<dyn TokenStore> = Arc::new(stores.token());
    let second: Arc<dyn TokenStore> = Arc::new(stores.clone().token());
    let family = unique("family");
    let old_hash = unique("old-hash");
    let new_hash = unique("new-hash");
    let reused_attempt = unique("reuse-hash");

    first
        .insert_refresh(RefreshRecord {
            token_hash: old_hash.clone(),
            family_id: family.clone(),
            subject: "user".to_owned(),
            custom: json!({ "role": "member" }),
            expires_at: 10_000,
            used_at: None,
            revoked: false,
        })
        .await
        .unwrap();

    let rotated = first
        .rotate_refresh(old_hash.clone(), new_hash.clone(), 11_000, 1_000)
        .await
        .unwrap();
    assert!(matches!(rotated, RotateRefresh::Rotated(_)));

    let reused = second
        .rotate_refresh(old_hash, reused_attempt, 12_000, 1_100)
        .await
        .unwrap();
    assert!(matches!(reused, RotateRefresh::Reused));
    assert!(
        first
            .is_family_revoked(family.clone(), 1_200)
            .await
            .unwrap()
    );
    assert!(
        second
            .find_refresh(new_hash, 1_200)
            .await
            .unwrap()
            .is_some_and(|record| record.revoked)
            || second.is_family_revoked(family, 1_200).await.unwrap()
    );
}

#[tokio::test]
async fn debug_redacts_url_password_when_connected() {
    let Some(url) = redis_url() else {
        return;
    };
    let Ok(stores) = RedisStores::connect(&url).await else {
        return;
    };
    let rendered = format!("{stores:?}");
    assert!(rendered.contains("RedisStores"));
    // Synthetic password URLs are unit-tested via `redact_redis_url`.
    assert!(!rendered.contains("s3cret"));
    let fake = "redis://tester:s3cret@127.0.0.1:6379/0";
    assert_eq!(
        phoenix_redis::redact_redis_url(fake),
        "redis://tester:***@127.0.0.1:6379/0"
    );
}
