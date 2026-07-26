//! Redis `pub/sub` implementation of the cross-instance realtime seam.
//!
//! [`RedisBroadcaster`] implements [`phoenix_http::Broadcaster`]: `publish`
//! becomes a Redis `PUBLISH` and `subscribe` a Redis `SUBSCRIBE` stream, so a
//! [`Hub`](phoenix_http::Hub) on one instance fans a channel broadcast (or an
//! identity-directed send) out to hubs on every other instance.
//!
//! ```ignore
//! let broadcaster = RedisBroadcaster::connect("redis://127.0.0.1").await?;
//! // Must be built inside a Tokio runtime: it spawns the inbound pump.
//! let hub = Hub::builder().broadcaster(broadcaster).build();
//! ```
//!
//! # Delivery semantics
//!
//! Redis `pub/sub` is **fire-and-forget**: it has no persistence, no acks, and
//! no replay. A message published while an instance is disconnected is gone for
//! that instance — it is not queued. That is the right trade for live fan-out
//! (a chat message or a presence blip is worthless late), and the wrong one for
//! anything that must not be lost; use `phoenix-queue` for those.
//!
//! Local delivery never depends on Redis: the [`Hub`](phoenix_http::Hub)
//! delivers to its own connections first and only then hands a copy here, so a
//! Redis outage degrades a cluster to independent single instances rather than
//! breaking realtime outright.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use phoenix_http::{Broadcaster, Bytes, HubId, Message, PeerFrame, PeerStream, PeerTarget};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};

use crate::RedisConnectError;
use crate::keys::BROADCAST_CHANNEL;

/// How long to wait before re-subscribing after the pub/sub stream drops.
const RESUBSCRIBE_BACKOFF: Duration = Duration::from_secs(1);

/// Cross-instance [`Broadcaster`] over Redis `pub/sub`.
///
/// Cheap to clone; clones share the same connection manager and Redis channel.
#[derive(Clone)]
pub struct RedisBroadcaster {
    inner: Arc<Inner>,
}

struct Inner {
    client: redis::Client,
    conn: ConnectionManager,
    channel: String,
    redacted_url: String,
}

impl std::fmt::Debug for RedisBroadcaster {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisBroadcaster")
            .field("url", &self.inner.redacted_url)
            .field("channel", &self.inner.channel)
            .finish_non_exhaustive()
    }
}

impl RedisBroadcaster {
    /// Connect using a Redis URL (`redis://...`) and the default channel.
    ///
    /// # Errors
    ///
    /// Returns an error when the URL is invalid or the initial connection
    /// fails.
    pub async fn connect(url: impl AsRef<str>) -> Result<Self, RedisConnectError> {
        let url = url.as_ref();
        let client = redis::Client::open(url).map_err(RedisConnectError::from_redis)?;
        let redacted = crate::keys::redact_redis_url(url);
        Self::from_client_with_label(client, redacted).await
    }

    /// Build from an existing [`redis::Client`].
    ///
    /// # Errors
    ///
    /// Returns an error when the connection manager cannot be established.
    pub async fn from_client(client: redis::Client) -> Result<Self, RedisConnectError> {
        let label = format!("{:?}", client.get_connection_info().addr);
        let redacted = crate::keys::redact_redis_url(&label);
        Self::from_client_with_label(client, redacted).await
    }

    async fn from_client_with_label(
        client: redis::Client,
        redacted_url: String,
    ) -> Result<Self, RedisConnectError> {
        let conn = ConnectionManager::new(client.clone())
            .await
            .map_err(RedisConnectError::from_redis)?;
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                conn,
                channel: BROADCAST_CHANNEL.to_owned(),
                redacted_url,
            }),
        })
    }

    /// Use a different Redis pub/sub channel.
    ///
    /// Every instance that should see each other's traffic must use the same
    /// channel; different channels give you isolated clusters on one Redis.
    #[must_use]
    pub fn channel(self, channel: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                client: self.inner.client.clone(),
                conn: self.inner.conn.clone(),
                channel: channel.into(),
                redacted_url: self.inner.redacted_url.clone(),
            }),
        }
    }

    /// The Redis pub/sub channel this broadcaster publishes on.
    #[must_use]
    pub fn channel_name(&self) -> &str {
        &self.inner.channel
    }
}

impl Broadcaster for RedisBroadcaster {
    fn publish(&self, frame: &PeerFrame) {
        let Ok(payload) = serde_json::to_string(&WireFrame::from(frame)) else {
            return;
        };
        let mut conn = self.inner.conn.clone();
        let channel = self.inner.channel.clone();
        // `Broadcaster::publish` is synchronous and must not block the hub's
        // lock-free fast path, so the round trip is detached. Redis pub/sub has
        // no delivery guarantee anyway: a failed publish is a dropped live
        // message, never a stuck hub.
        tokio::spawn(async move {
            let _: Result<i64, _> = conn.publish(channel, payload).await;
        });
    }

    fn subscribe(&self) -> Option<PeerStream> {
        let (sender, receiver) = tokio::sync::mpsc::channel::<PeerFrame>(256);
        let client = self.inner.client.clone();
        let channel = self.inner.channel.clone();
        tokio::spawn(async move {
            // Reconnect for as long as anything is listening. Dropping the hub
            // closes the receiver, which ends this task.
            while !sender.is_closed() {
                if let Ok(mut pubsub) = client.get_async_pubsub().await
                    && pubsub.subscribe(&channel).await.is_ok()
                {
                    let mut stream = pubsub.into_on_message();
                    while let Some(message) = stream.next().await {
                        let Ok(payload) = message.get_payload::<String>() else {
                            continue;
                        };
                        let Ok(frame) = serde_json::from_str::<WireFrame>(&payload) else {
                            // A malformed or newer-format payload is skipped,
                            // never fatal to the pump.
                            continue;
                        };
                        if sender.send(frame.into()).await.is_err() {
                            return;
                        }
                    }
                }
                tokio::time::sleep(RESUBSCRIBE_BACKOFF).await;
            }
        });
        Some(Box::pin(tokio_stream_receiver(receiver)))
    }
}

/// Adapt an mpsc receiver into the `Stream` the hub expects.
fn tokio_stream_receiver(
    receiver: tokio::sync::mpsc::Receiver<PeerFrame>,
) -> impl futures_util::Stream<Item = PeerFrame> + Send + 'static {
    futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|frame| (frame, receiver))
    })
}

/// The JSON shape published on Redis.
///
/// Written out explicitly rather than derived on the `phoenix-http` types: this
/// is a wire format shared between processes (and potentially between Phoenix
/// versions), so it must not silently follow refactors of the in-memory enums.
#[derive(Debug, Serialize, Deserialize)]
struct WireFrame {
    /// Originating hub id, used to skip self-echoes.
    origin: u64,
    /// `"channel"` or `"key"`.
    target: WireTarget,
    /// The message payload.
    message: WireMessage,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireTarget {
    Channel { name: String },
    Key { key: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireMessage {
    Text {
        text: String,
    },
    /// Base64 so the frame stays valid UTF-8 JSON.
    Binary {
        base64: String,
    },
}

impl From<&PeerFrame> for WireFrame {
    fn from(frame: &PeerFrame) -> Self {
        Self {
            origin: frame.origin.get(),
            target: match &frame.target {
                PeerTarget::Channel(name) => WireTarget::Channel { name: name.clone() },
                PeerTarget::Key(key) => WireTarget::Key { key: key.clone() },
            },
            message: WireMessage::from(&frame.message),
        }
    }
}

impl From<WireFrame> for PeerFrame {
    fn from(frame: WireFrame) -> Self {
        Self {
            origin: HubId::from_raw(frame.origin),
            target: match frame.target {
                WireTarget::Channel { name } => PeerTarget::Channel(name),
                WireTarget::Key { key } => PeerTarget::Key(key),
            },
            message: frame.message.into(),
        }
    }
}

impl From<&Message> for WireMessage {
    fn from(message: &Message) -> Self {
        match message {
            Message::Text(text) => Self::Text { text: text.clone() },
            Message::Binary(bytes) => Self::Binary {
                base64: base64_encode(bytes),
            },
            // Control frames are per-connection liveness signals with no
            // meaning on another node, so they are replicated as an empty
            // binary payload rather than silently forwarded as themselves.
            Message::Ping(_) | Message::Pong(_) | Message::Close(_) => Self::Binary {
                base64: String::new(),
            },
        }
    }
}

impl From<WireMessage> for Message {
    fn from(message: WireMessage) -> Self {
        match message {
            WireMessage::Text { text } => Self::Text(text),
            WireMessage::Binary { base64 } => {
                Self::Binary(Bytes::from(base64_decode(&base64).unwrap_or_default()))
            }
        }
    }
}

/// Standard base64 (`RFC 4648`) encode — kept local so this crate does not grow
/// a dependency for one field.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Standard base64 decode; `None` for any invalid input.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut triple = 0_u32;
        for (index, byte) in chunk.iter().enumerate() {
            let six = if *byte == b'=' { 0 } else { value(*byte)? };
            triple |= six << (18 - 6 * index);
        }
        out.push(((triple >> 16) & 0xFF) as u8);
        if pad < 2 {
            out.push(((triple >> 8) & 0xFF) as u8);
        }
        if pad < 1 {
            out.push((triple & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_length_class() {
        for payload in [
            b"".as_slice(),
            b"a",
            b"ab",
            b"abc",
            b"abcd",
            &[0, 255, 128, 1, 2, 3, 250],
        ] {
            let encoded = base64_encode(payload);
            assert_eq!(encoded.len() % 4, 0, "padded to a multiple of four");
            assert_eq!(
                base64_decode(&encoded).as_deref(),
                Some(payload),
                "round trip for {payload:?}"
            );
        }
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert!(base64_decode("YWJ").is_none(), "bad length");
        assert!(base64_decode("YW!j").is_none(), "bad alphabet");
    }

    fn frame(target: PeerTarget, message: Message) -> PeerFrame {
        PeerFrame {
            origin: HubId::from_raw(0x1234_5678_9ABC_DEF0),
            target,
            message,
        }
    }

    #[test]
    fn wire_format_round_trips_channel_and_key_targets() {
        for original in [
            frame(
                PeerTarget::Channel("room:1".to_owned()),
                Message::text("hello"),
            ),
            frame(
                PeerTarget::Key("alice".to_owned()),
                Message::binary(vec![0_u8, 1, 2, 255]),
            ),
        ] {
            let json = serde_json::to_string(&WireFrame::from(&original)).unwrap();
            let decoded: PeerFrame = serde_json::from_str::<WireFrame>(&json).unwrap().into();
            assert_eq!(decoded, original);
        }
    }

    #[test]
    fn the_wire_shape_is_pinned() {
        // A cross-process format: changing these names breaks a rolling deploy
        // mid-flight, so the shape is asserted rather than left implicit.
        let json = serde_json::to_value(WireFrame::from(&frame(
            PeerTarget::Channel("room:1".to_owned()),
            Message::text("hi"),
        )))
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "origin": 0x1234_5678_9ABC_DEF0_u64,
                "target": { "kind": "channel", "name": "room:1" },
                "message": { "type": "text", "text": "hi" },
            })
        );
    }

    #[test]
    fn control_frames_do_not_cross_instances_as_themselves() {
        // Ping/Pong/Close are per-socket liveness signals; replaying one on
        // another node's socket would be meaningless at best.
        let decoded: PeerFrame = serde_json::from_str::<WireFrame>(
            &serde_json::to_string(&WireFrame::from(&frame(
                PeerTarget::Channel("room:1".to_owned()),
                Message::Ping(Bytes::from_static(b"beat")),
            )))
            .unwrap(),
        )
        .unwrap()
        .into();
        assert_eq!(decoded.message, Message::Binary(Bytes::new()));
    }

    #[test]
    fn malformed_payloads_are_rejected_not_guessed() {
        for payload in [
            "not json",
            r#"{"origin":1}"#,
            r#"{"origin":1,"target":{"kind":"nope"},"message":{"type":"text","text":"x"}}"#,
            r#"{"origin":1,"target":{"kind":"channel","name":"c"},"message":{"type":"video"}}"#,
        ] {
            assert!(
                serde_json::from_str::<WireFrame>(payload).is_err(),
                "{payload} must not parse"
            );
        }
    }
}
