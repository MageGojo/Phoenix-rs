//! Cross-instance realtime contracts for [`RedisBroadcaster`], gated by
//! `PHOENIX_TEST_REDIS_URL`.
//!
//! Two [`Hub`]s in this process, each with its own broadcaster over a real
//! Redis, stand in for two server instances. What is verified:
//!
//! - a channel broadcast on one instance reaches subscribers on the other;
//! - an identity-directed send crosses the same way and reaches only that
//!   identity;
//! - the originating hub never double-delivers its own frame;
//! - two clusters on separate Redis channels do not see each other;
//! - binary payloads survive the JSON wire format byte for byte.

use std::time::Duration;

use phoenix_http::{ConnectionMeta, Hub, Message, Outbound, Outgoing};
use phoenix_redis::RedisBroadcaster;

fn redis_url() -> Option<String> {
    std::env::var("PHOENIX_TEST_REDIS_URL")
        .ok()
        .filter(|value| !value.is_empty())
}

async fn broadcaster(channel: &str) -> Option<RedisBroadcaster> {
    let url = redis_url()?;
    match RedisBroadcaster::connect(&url).await {
        Ok(broadcaster) => Some(broadcaster.channel(channel.to_owned())),
        Err(error) => {
            eprintln!("skipping redis broadcast integration: {error}");
            None
        }
    }
}

fn unique(prefix: &str) -> String {
    format!(
        "phoenix-test:{prefix}:{}:{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    )
}

/// Wait for one application message, or `None` if none arrives in time.
///
/// Redis pub/sub delivery is asynchronous, so a bounded wait is the honest
/// assertion; a plain `try_recv` would be racy.
async fn next_message(outbound: &mut Outbound) -> Option<Message> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, outbound.recv()).await {
            Ok(Some(Outgoing::Message(message))) => return Some(message),
            Ok(Some(Outgoing::Presence(_))) => {}
            Ok(None) | Err(_) => return None,
        }
    }
}

/// Assert nothing arrives within a short grace period.
async fn expect_silence(outbound: &mut Outbound) {
    let quiet = tokio::time::timeout(Duration::from_millis(750), outbound.recv()).await;
    assert!(
        quiet.is_err(),
        "expected no delivery, got {:?}",
        quiet.ok().flatten()
    );
}

#[tokio::test]
async fn channel_broadcasts_reach_a_peer_instance() {
    let channel = unique("bus");
    let (Some(bus_a), Some(bus_b)) = (broadcaster(&channel).await, broadcaster(&channel).await)
    else {
        return;
    };
    let hub_a = Hub::builder().broadcaster(bus_a).build();
    let hub_b = Hub::builder().broadcaster(bus_b).build();
    // Give both pub/sub pumps time to finish SUBSCRIBE before publishing.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let room = unique("room");
    let (a, mut a_out) = hub_a.connect();
    let (b, mut b_out) = hub_b.connect();
    a.join(&room).unwrap();
    b.join(&room).unwrap();

    hub_a.broadcast(&room, Message::text("cross-instance"));

    assert_eq!(
        next_message(&mut a_out).await,
        Some(Message::text("cross-instance")),
        "local delivery does not wait on Redis"
    );
    assert_eq!(
        next_message(&mut b_out).await,
        Some(Message::text("cross-instance")),
        "the peer instance received the published frame"
    );
    // The originating hub must not replay its own frame off the bus.
    expect_silence(&mut a_out).await;
    drop((a, b));
}

#[tokio::test]
async fn identity_directed_sends_reach_only_that_identity() {
    let channel = unique("bus");
    let (Some(bus_a), Some(bus_b)) = (broadcaster(&channel).await, broadcaster(&channel).await)
    else {
        return;
    };
    let hub_a = Hub::builder().broadcaster(bus_a).build();
    let hub_b = Hub::builder().broadcaster(bus_b).build();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let alice = unique("alice");
    let (here, mut here_out) = hub_a.connect_as(ConnectionMeta::new().with_key(alice.clone()));
    let (elsewhere, mut elsewhere_out) =
        hub_b.connect_as(ConnectionMeta::new().with_key(alice.clone()));
    let (bob, mut bob_out) = hub_b.connect_as(ConnectionMeta::new().with_key(unique("bob")));

    let delivered = hub_a.send_to_key(&alice, Message::text("private"));
    assert_eq!(delivered, 1, "one local connection for this identity");

    assert_eq!(
        next_message(&mut here_out).await,
        Some(Message::text("private"))
    );
    assert_eq!(
        next_message(&mut elsewhere_out).await,
        Some(Message::text("private")),
        "the same identity on another instance is reached"
    );
    expect_silence(&mut bob_out).await;
    drop((here, elsewhere, bob));
}

#[tokio::test]
async fn separate_redis_channels_are_isolated_clusters() {
    let (Some(bus_a), Some(bus_b)) = (
        broadcaster(&unique("bus-one")).await,
        broadcaster(&unique("bus-two")).await,
    ) else {
        return;
    };
    let hub_a = Hub::builder().broadcaster(bus_a).build();
    let hub_b = Hub::builder().broadcaster(bus_b).build();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let room = unique("room");
    let (a, _a_out) = hub_a.connect();
    let (b, mut b_out) = hub_b.connect();
    a.join(&room).unwrap();
    b.join(&room).unwrap();

    hub_a.broadcast(&room, Message::text("not-for-you"));

    expect_silence(&mut b_out).await;
    drop((a, b));
}

#[tokio::test]
async fn binary_payloads_survive_the_wire_format() {
    let channel = unique("bus");
    let (Some(bus_a), Some(bus_b)) = (broadcaster(&channel).await, broadcaster(&channel).await)
    else {
        return;
    };
    let hub_a = Hub::builder().broadcaster(bus_a).build();
    let hub_b = Hub::builder().broadcaster(bus_b).build();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let room = unique("room");
    let (a, _a_out) = hub_a.connect();
    let (b, mut b_out) = hub_b.connect();
    a.join(&room).unwrap();
    b.join(&room).unwrap();

    // Every byte class, including non-UTF-8 sequences the JSON envelope must
    // not mangle.
    let payload: Vec<u8> = (0..=u8::MAX).collect();
    hub_a.broadcast(&room, Message::binary(payload.clone()));

    assert_eq!(
        next_message(&mut b_out).await,
        Some(Message::binary(payload))
    );
    drop((a, b));
}
