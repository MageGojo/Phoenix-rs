//! In-process broadcast hub, named channels, and presence for WebSocket
//! connections.
//!
//! The [`Hub`] sits above the per-connection [`WebSocket`](crate::WebSocket)
//! facade in `ws.rs`. It owns a registry of connections, fans messages out to
//! named channels, tracks channel membership (presence), and exposes an
//! application-supplied authorization hook.
//!
//! # Outbound model and backpressure
//!
//! Every connection owns one bounded `tokio::sync::mpsc` queue. Directed sends,
//! channel broadcasts, and presence events all funnel through that single
//! queue, which is the one backpressure point per connection. When a queue is
//! full the configured [`SlowConsumer`] policy decides whether the message is
//! dropped or the connection is evicted. A per-connection `mpsc` (rather than a
//! shared `tokio::sync::broadcast`) is used deliberately: it is the only shape
//! that supports directed delivery to a single connection, keeps slow-consumer
//! effects isolated to the offending connection, and makes the policy explicit.
//!
//! # Cross-instance seam
//!
//! Local fan-out is always performed by the [`Hub`]. The [`Broadcaster`] trait
//! is the seam for replicating messages to *other* server instances. This crate
//! ships only [`LocalBroadcaster`] (single instance, no peers); the Redis
//! `pub/sub` implementation lives in `phoenix-redis`
//! (`RedisBroadcaster`) and must not be added here.
//!
//! Two things cross the seam: channel broadcasts ([`PeerTarget::Channel`]) and
//! identity-directed sends ([`PeerTarget::Key`]). Raw [`ConnectionId`]s do not:
//! they are per-hub handles with no meaning on another node, so directed
//! cross-instance delivery is addressed by the application identity in
//! [`ConnectionMeta::key`] instead — see [`Hub::send_to_key`].

#![allow(clippy::module_name_repetitions)]

use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::{Stream, StreamExt};
use thiserror::Error;
use tokio::sync::mpsc::{self, error::TrySendError};

use crate::ws::Message;

static NEXT_HUB_ID: AtomicU64 = AtomicU64::new(1);

/// Mint a hub id that is unique across *processes*, not just within one.
///
/// The cross-instance echo filter drops frames whose origin equals the local
/// hub id, so two instances that both numbered their hubs `1` would silently
/// discard each other's traffic. Mixing a per-process seed (start time and pid)
/// into a counter makes a collision a practical impossibility without pulling a
/// random-number generator into this crate.
fn next_hub_id() -> u64 {
    static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let seed = *SEED.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            // Truncating to the low 64 bits is fine: this is seed entropy, not
            // a timestamp anyone reads back.
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_nanos() & u128::from(u64::MAX)).unwrap_or(0)
            });
        splitmix64(nanos ^ (u64::from(std::process::id()) << 32))
    });
    splitmix64(seed ^ NEXT_HUB_ID.fetch_add(1, Ordering::Relaxed))
}

/// `SplitMix64` finalizer: cheap avalanche so neighbouring inputs stay far apart.
const fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Identifier for a single registered connection within a [`Hub`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// The raw numeric identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Identifier for a [`Hub`] instance, used to skip a hub's own peer echoes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HubId(u64);

impl HubId {
    /// The raw numeric identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Rebuild an id from the wire.
    ///
    /// Cross-instance transports carry [`PeerFrame::origin`] as a number and
    /// must hand it back unchanged; only a faithful round trip lets a hub
    /// recognize (and skip) its own echoes.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// How a full per-connection outbound queue is handled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlowConsumer {
    /// Evict the connection when its outbound queue is full. This closes the
    /// connection's [`Outbound`] receiver so its socket pump stops. Preferred
    /// when message continuity matters more than keeping the socket open, since
    /// a lagging consumer would otherwise silently miss messages.
    Disconnect,
    /// Drop the overflowing message and keep the connection. Lossy, but never
    /// evicts. Suitable for best-effort telemetry-style fan-out.
    DropMessage,
}

/// Tuning for a [`Hub`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HubConfig {
    /// Depth of each connection's bounded outbound queue (clamped to `>= 1`).
    pub capacity: usize,
    /// Slow-consumer policy applied when an outbound queue is full.
    pub slow_consumer: SlowConsumer,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            capacity: 64,
            slow_consumer: SlowConsumer::Disconnect,
        }
    }
}

/// Application-supplied metadata for a connection, surfaced through presence and
/// the [`Authorizer`] hook.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConnectionMeta {
    /// Stable application identity (for example a user id). When absent,
    /// presence falls back to a `conn:<id>` key.
    pub key: Option<String>,
    /// Optional opaque presence state (for example a serialized status blob).
    pub state: Option<String>,
}

impl ConnectionMeta {
    /// An empty metadata value.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the application identity key.
    #[must_use]
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Set the opaque presence state.
    #[must_use]
    pub fn with_state(mut self, state: impl Into<String>) -> Self {
        self.state = Some(state.into());
        self
    }
}

/// A member currently present in a channel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresenceMember {
    /// The connection backing this member.
    pub connection: ConnectionId,
    /// The application identity key (from [`ConnectionMeta::key`], or `conn:<id>`).
    pub key: String,
    /// The opaque presence state, if any.
    pub state: Option<String>,
}

/// Whether a presence event marks a member joining or leaving.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresenceEventKind {
    /// A member joined the channel.
    Join,
    /// A member left the channel (via leave, disconnect, or eviction).
    Leave,
}

/// A membership change delivered to the other members of a channel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresenceEvent {
    /// The affected channel.
    pub channel: String,
    /// Whether the member joined or left.
    pub kind: PresenceEventKind,
    /// The member whose membership changed.
    pub member: PresenceMember,
}

/// An item delivered on a connection's outbound queue.
///
/// The socket pump turns this into wire frames. Presence events are kept as a
/// distinct variant instead of a hard-coded wire format so the application
/// chooses how to serialize them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outgoing {
    /// An application message (a directed send or a channel broadcast).
    Message(Message),
    /// A channel membership change.
    Presence(PresenceEvent),
}

/// The receiving half of a connection's outbound queue.
///
/// A socket pump drains this and writes each item to the
/// [`WebSocket`](crate::WebSocket). `recv` returning `None` means the connection
/// was evicted or its sender was dropped; the pump should then close the socket.
#[derive(Debug)]
pub struct Outbound {
    receiver: mpsc::Receiver<Outgoing>,
}

impl Outbound {
    /// Await the next outbound item. `None` means the connection is finished.
    pub async fn recv(&mut self) -> Option<Outgoing> {
        self.receiver.recv().await
    }

    /// Take a buffered item without waiting. `None` means empty or closed.
    #[must_use]
    pub fn try_recv(&mut self) -> Option<Outgoing> {
        self.receiver.try_recv().ok()
    }
}

/// Context passed to the [`Authorizer`] when a connection tries to join a channel.
#[derive(Clone, Copy, Debug)]
pub struct ConnectionContext<'a> {
    id: ConnectionId,
    meta: &'a ConnectionMeta,
}

impl ConnectionContext<'_> {
    /// The joining connection's identifier.
    #[must_use]
    pub const fn id(&self) -> ConnectionId {
        self.id
    }

    /// The connection's application identity key, if any.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        self.meta.key.as_deref()
    }

    /// The connection's opaque presence state, if any.
    #[must_use]
    pub fn state(&self) -> Option<&str> {
        self.meta.state.as_deref()
    }
}

/// Authorization hook consulted before a connection joins a channel.
///
/// The default [`AllowAll`] permits every join. Applications override this to
/// enforce their own policy; the framework never hard-codes authorization. A
/// bare closure `Fn(&str, &ConnectionContext) -> bool` also implements this
/// trait.
pub trait Authorizer: Send + Sync + 'static {
    /// Return `true` to allow `ctx` to join `channel`.
    fn authorize(&self, channel: &str, ctx: &ConnectionContext<'_>) -> bool;
}

/// An [`Authorizer`] that permits every join. This is the default.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAll;

impl Authorizer for AllowAll {
    fn authorize(&self, _channel: &str, _ctx: &ConnectionContext<'_>) -> bool {
        true
    }
}

impl<F> Authorizer for F
where
    F: Fn(&str, &ConnectionContext<'_>) -> bool + Send + Sync + 'static,
{
    fn authorize(&self, channel: &str, ctx: &ConnectionContext<'_>) -> bool {
        self(channel, ctx)
    }
}

/// Who a replicated message is for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerTarget {
    /// Every member of a named channel.
    Channel(String),
    /// Every connection whose [`ConnectionMeta::key`] matches — the
    /// cross-instance form of a directed send. Raw [`ConnectionId`]s are
    /// deliberately not addressable across instances: they are per-hub handles.
    Key(String),
}

impl PeerTarget {
    /// The channel name, for a channel-targeted frame.
    #[must_use]
    pub fn channel(&self) -> Option<&str> {
        match self {
            Self::Channel(channel) => Some(channel),
            Self::Key(_) => None,
        }
    }

    /// The identity key, for a key-targeted frame.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        match self {
            Self::Key(key) => Some(key),
            Self::Channel(_) => None,
        }
    }
}

/// A message as it crosses the cross-instance [`Broadcaster`] seam.
///
/// Transports must carry [`PeerFrame::origin`] on the wire so a hub can skip its
/// own echoes and avoid a delivery loop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerFrame {
    /// The hub that originally published the message.
    pub origin: HubId,
    /// Who the message is for.
    pub target: PeerTarget,
    /// The message payload.
    pub message: Message,
}

/// A stream of [`PeerFrame`]s published by *other* instances.
pub type PeerStream = Pin<Box<dyn Stream<Item = PeerFrame> + Send + 'static>>;

/// The cross-instance replication seam.
///
/// The [`Hub`] always delivers locally; this trait forwards a copy to peer
/// instances and streams peers' messages back. Only [`LocalBroadcaster`] ships
/// today; a Redis `pub/sub` implementation is phase 2 and belongs in
/// `phoenix-redis`, not here.
pub trait Broadcaster: Send + Sync + 'static {
    /// Forward a locally-published frame to peer instances.
    ///
    /// Local delivery is handled by the [`Hub`] regardless; this is purely the
    /// peer fan-out seam.
    fn publish(&self, frame: &PeerFrame);

    /// A stream of frames published by other instances, or `None` for a
    /// single-instance deployment with no peers (the [`Hub`] then skips the
    /// inbound pump). A hub built with a peer-backed broadcaster must be
    /// constructed inside a Tokio runtime.
    fn subscribe(&self) -> Option<PeerStream> {
        None
    }
}

/// The single-instance [`Broadcaster`]. `publish` is a no-op and there are no
/// peers, so all delivery is local.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalBroadcaster;

impl Broadcaster for LocalBroadcaster {
    fn publish(&self, _frame: &PeerFrame) {}
}

/// Why a channel join was refused.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum JoinError {
    /// The [`Authorizer`] denied the join.
    #[error("The connection is not authorized to join this channel.")]
    Unauthorized,
    /// The connection is no longer registered (dropped or evicted).
    #[error("The connection is closed.")]
    ConnectionClosed,
}

/// Why a directed send failed.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SendError {
    /// No connection is registered with the given identifier.
    #[error("No connection is registered with this identifier.")]
    UnknownConnection,
    /// The connection's outbound queue was full; the message was not delivered.
    /// Under [`SlowConsumer::Disconnect`] the connection is also evicted.
    #[error("The connection's outbound queue is full.")]
    Backpressure,
    /// The connection's [`Outbound`] receiver has been dropped.
    #[error("The connection's receiver has been dropped.")]
    Closed,
}

#[derive(Debug)]
struct ConnectionEntry {
    sender: mpsc::Sender<Outgoing>,
    meta: ConnectionMeta,
    joined: HashSet<String>,
}

#[derive(Debug, Default)]
struct ChannelState {
    members: HashSet<ConnectionId>,
}

#[derive(Debug, Default)]
struct HubState {
    connections: HashMap<ConnectionId, ConnectionEntry>,
    channels: HashMap<String, ChannelState>,
}

struct HubInner {
    id: HubId,
    next_connection: AtomicU64,
    config: HubConfig,
    authorizer: Arc<dyn Authorizer>,
    broadcaster: Arc<dyn Broadcaster>,
    state: Mutex<HubState>,
}

impl HubInner {
    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn connect(&self, meta: ConnectionMeta) -> (ConnectionId, mpsc::Receiver<Outgoing>) {
        let id = ConnectionId(self.next_connection.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = mpsc::channel(self.config.capacity.max(1));
        let mut guard = self.lock();
        guard.connections.insert(
            id,
            ConnectionEntry {
                sender,
                meta,
                joined: HashSet::new(),
            },
        );
        (id, receiver)
    }

    fn join(&self, id: ConnectionId, channel: &str) -> Result<(), JoinError> {
        let mut guard = self.lock();
        let state = &mut *guard;

        let meta = match state.connections.get(&id) {
            Some(entry) => entry.meta.clone(),
            None => return Err(JoinError::ConnectionClosed),
        };
        let context = ConnectionContext { id, meta: &meta };
        if !self.authorizer.authorize(channel, &context) {
            return Err(JoinError::Unauthorized);
        }

        let newly_joined = match state.connections.get_mut(&id) {
            Some(entry) => entry.joined.insert(channel.to_owned()),
            None => return Err(JoinError::ConnectionClosed),
        };
        if !newly_joined {
            return Ok(());
        }

        let others: Vec<ConnectionId> = {
            let chan = state.channels.entry(channel.to_owned()).or_default();
            chan.members.insert(id);
            chan.members.iter().copied().filter(|m| *m != id).collect()
        };

        let event = Outgoing::Presence(PresenceEvent {
            channel: channel.to_owned(),
            kind: PresenceEventKind::Join,
            member: presence_member(id, &meta),
        });
        for member in others {
            if let Some(entry) = state.connections.get(&member) {
                let _ = entry.sender.try_send(event.clone());
            }
        }
        Ok(())
    }

    fn leave(&self, id: ConnectionId, channel: &str) {
        let mut guard = self.lock();
        let state = &mut *guard;

        if let Some(entry) = state.connections.get_mut(&id) {
            entry.joined.remove(channel);
        }
        let meta = state.connections.get(&id).map(|entry| entry.meta.clone());

        let (removed, empty, others) = match state.channels.get_mut(channel) {
            Some(chan) => {
                let removed = chan.members.remove(&id);
                let empty = chan.members.is_empty();
                let others: Vec<ConnectionId> = chan.members.iter().copied().collect();
                (removed, empty, others)
            }
            None => (false, false, Vec::new()),
        };

        if !removed {
            return;
        }
        if let Some(meta) = meta {
            let member = presence_member(id, &meta);
            for other in others {
                if let Some(entry) = state.connections.get(&other) {
                    let _ = entry.sender.try_send(Outgoing::Presence(PresenceEvent {
                        channel: channel.to_owned(),
                        kind: PresenceEventKind::Leave,
                        member: member.clone(),
                    }));
                }
            }
        }
        if empty {
            state.channels.remove(channel);
        }
    }

    fn disconnect(&self, id: ConnectionId) {
        let mut guard = self.lock();
        evict(&mut guard, id);
    }

    fn send_to(&self, id: ConnectionId, message: Message) -> Result<(), SendError> {
        let mut guard = self.lock();
        let state = &mut *guard;
        let outcome = match state.connections.get(&id) {
            Some(entry) => entry.sender.try_send(Outgoing::Message(message)),
            None => return Err(SendError::UnknownConnection),
        };
        match outcome {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                if self.config.slow_consumer == SlowConsumer::Disconnect {
                    evict(state, id);
                }
                Err(SendError::Backpressure)
            }
            Err(TrySendError::Closed(_)) => {
                evict(state, id);
                Err(SendError::Closed)
            }
        }
    }

    fn broadcast(&self, channel: &str, message: Message) {
        {
            let mut guard = self.lock();
            self.broadcast_local(&mut guard, channel, &message);
        }
        self.broadcaster.publish(&PeerFrame {
            origin: self.id,
            target: PeerTarget::Channel(channel.to_owned()),
            message,
        });
    }

    /// Deliver to every local connection whose identity key matches, and
    /// return how many were reached.
    fn send_to_key_local(&self, state: &mut HubState, key: &str, message: &Message) -> usize {
        let targets: Vec<ConnectionId> = state
            .connections
            .iter()
            .filter(|(_, entry)| entry.meta.key.as_deref() == Some(key))
            .map(|(id, _)| *id)
            .collect();
        let mut delivered = 0;
        let mut to_evict = Vec::new();
        for id in targets {
            let Some(entry) = state.connections.get(&id) else {
                continue;
            };
            match entry.sender.try_send(Outgoing::Message(message.clone())) {
                Ok(()) => delivered += 1,
                Err(TrySendError::Full(_)) => {
                    if self.config.slow_consumer == SlowConsumer::Disconnect {
                        to_evict.push(id);
                    }
                }
                Err(TrySendError::Closed(_)) => to_evict.push(id),
            }
        }
        for id in to_evict {
            evict(state, id);
        }
        delivered
    }

    fn send_to_key(&self, key: &str, message: Message) -> usize {
        let delivered = {
            let mut guard = self.lock();
            self.send_to_key_local(&mut guard, key, &message)
        };
        self.broadcaster.publish(&PeerFrame {
            origin: self.id,
            target: PeerTarget::Key(key.to_owned()),
            message,
        });
        delivered
    }

    fn broadcast_local(&self, state: &mut HubState, channel: &str, message: &Message) {
        let members: Vec<ConnectionId> = match state.channels.get(channel) {
            Some(chan) => chan.members.iter().copied().collect(),
            None => return,
        };
        let mut to_evict = Vec::new();
        for id in members {
            let Some(entry) = state.connections.get(&id) else {
                continue;
            };
            match entry.sender.try_send(Outgoing::Message(message.clone())) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    if self.config.slow_consumer == SlowConsumer::Disconnect {
                        to_evict.push(id);
                    }
                }
                Err(TrySendError::Closed(_)) => to_evict.push(id),
            }
        }
        for id in to_evict {
            evict(state, id);
        }
    }

    fn deliver_from_peer(&self, frame: &PeerFrame) {
        if frame.origin == self.id {
            return;
        }
        let mut guard = self.lock();
        match &frame.target {
            PeerTarget::Channel(channel) => {
                self.broadcast_local(&mut guard, channel, &frame.message);
            }
            PeerTarget::Key(key) => {
                self.send_to_key_local(&mut guard, key, &frame.message);
            }
        }
    }

    fn presence(&self, channel: &str) -> Vec<PresenceMember> {
        let guard = self.lock();
        let Some(chan) = guard.channels.get(channel) else {
            return Vec::new();
        };
        chan.members
            .iter()
            .filter_map(|id| {
                guard
                    .connections
                    .get(id)
                    .map(|entry| presence_member(*id, &entry.meta))
            })
            .collect()
    }

    fn member_count(&self, channel: &str) -> usize {
        self.lock()
            .channels
            .get(channel)
            .map_or(0, |chan| chan.members.len())
    }

    fn connection_channels(&self, id: ConnectionId) -> Vec<String> {
        let mut channels = self
            .lock()
            .connections
            .get(&id)
            .map(|entry| entry.joined.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        channels.sort();
        channels
    }
}

fn presence_member(id: ConnectionId, meta: &ConnectionMeta) -> PresenceMember {
    PresenceMember {
        connection: id,
        key: meta.key.clone().unwrap_or_else(|| format!("conn:{}", id.0)),
        state: meta.state.clone(),
    }
}

/// Remove a connection and emit best-effort `Leave` events to remaining members.
///
/// Presence events are delivered best-effort (dropped if a recipient's queue is
/// full) so eviction never cascades into further evictions.
fn evict(state: &mut HubState, id: ConnectionId) {
    let Some(entry) = state.connections.remove(&id) else {
        return;
    };
    let member = presence_member(id, &entry.meta);
    for channel in &entry.joined {
        let Some(chan) = state.channels.get_mut(channel) else {
            continue;
        };
        if !chan.members.remove(&id) {
            continue;
        }
        let empty = chan.members.is_empty();
        let others: Vec<ConnectionId> = chan.members.iter().copied().collect();
        for other in others {
            if let Some(recipient) = state.connections.get(&other) {
                let _ = recipient.sender.try_send(Outgoing::Presence(PresenceEvent {
                    channel: channel.clone(),
                    kind: PresenceEventKind::Leave,
                    member: member.clone(),
                }));
            }
        }
        if empty {
            state.channels.remove(channel);
        }
    }
}

/// Builder for a configured [`Hub`].
pub struct HubBuilder {
    config: HubConfig,
    authorizer: Arc<dyn Authorizer>,
    broadcaster: Arc<dyn Broadcaster>,
}

impl Default for HubBuilder {
    fn default() -> Self {
        Self {
            config: HubConfig::default(),
            authorizer: Arc::new(AllowAll),
            broadcaster: Arc::new(LocalBroadcaster),
        }
    }
}

impl std::fmt::Debug for HubBuilder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HubBuilder")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl HubBuilder {
    /// Set the per-connection outbound queue depth (clamped to `>= 1`).
    #[must_use]
    pub fn capacity(mut self, capacity: usize) -> Self {
        self.config.capacity = capacity.max(1);
        self
    }

    /// Set the slow-consumer policy.
    #[must_use]
    pub fn slow_consumer(mut self, policy: SlowConsumer) -> Self {
        self.config.slow_consumer = policy;
        self
    }

    /// Install a channel authorization hook. A closure
    /// `Fn(&str, &ConnectionContext) -> bool` is accepted.
    #[must_use]
    pub fn authorizer(mut self, authorizer: impl Authorizer) -> Self {
        self.authorizer = Arc::new(authorizer);
        self
    }

    /// Install a cross-instance [`Broadcaster`]. Defaults to [`LocalBroadcaster`].
    #[must_use]
    pub fn broadcaster(mut self, broadcaster: impl Broadcaster) -> Self {
        self.broadcaster = Arc::new(broadcaster);
        self
    }

    /// Build the [`Hub`].
    ///
    /// If the broadcaster exposes a peer subscription, an inbound pump task is
    /// spawned, so this must run inside a Tokio runtime for peer-backed
    /// broadcasters. [`LocalBroadcaster`] spawns nothing.
    #[must_use]
    pub fn build(self) -> Hub {
        let inner = Arc::new(HubInner {
            id: HubId(next_hub_id()),
            next_connection: AtomicU64::new(1),
            config: self.config,
            authorizer: self.authorizer,
            broadcaster: self.broadcaster,
            state: Mutex::new(HubState::default()),
        });
        if let Some(stream) = inner.broadcaster.subscribe() {
            spawn_peer_pump(&inner, stream);
        }
        Hub(inner)
    }
}

fn spawn_peer_pump(inner: &Arc<HubInner>, mut stream: PeerStream) {
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        while let Some(frame) = stream.next().await {
            match weak.upgrade() {
                Some(inner) => inner.deliver_from_peer(&frame),
                None => break,
            }
        }
    });
}

/// A process-wide broadcast hub shared by many connections.
///
/// Cheap to clone (an `Arc` handle). See the [module docs](self) for the
/// backpressure and cross-instance model.
#[derive(Clone)]
pub struct Hub(Arc<HubInner>);

impl std::fmt::Debug for Hub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Hub")
            .field("id", &self.0.id)
            .field("config", &self.0.config)
            .finish_non_exhaustive()
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    /// Create a hub with default configuration ([`AllowAll`], [`LocalBroadcaster`]).
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Start building a configured hub.
    #[must_use]
    pub fn builder() -> HubBuilder {
        HubBuilder::default()
    }

    /// This hub's identifier.
    #[must_use]
    pub fn id(&self) -> HubId {
        self.0.id
    }

    /// Register a connection with default (empty) metadata.
    #[must_use]
    pub fn connect(&self) -> (Connection, Outbound) {
        self.connect_as(ConnectionMeta::default())
    }

    /// Register a connection with the given metadata (identity and presence state).
    #[must_use]
    pub fn connect_as(&self, meta: ConnectionMeta) -> (Connection, Outbound) {
        let (id, receiver) = self.0.connect(meta);
        (
            Connection {
                hub: self.clone(),
                id,
            },
            Outbound { receiver },
        )
    }

    /// Broadcast a message to every member of `channel` and forward it to peers.
    pub fn broadcast(&self, channel: &str, message: Message) {
        self.0.broadcast(channel, message);
    }

    /// Send a message to a single connection.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::UnknownConnection`] if no such connection is
    /// registered, [`SendError::Backpressure`] if its queue is full, or
    /// [`SendError::Closed`] if its receiver was dropped.
    pub fn send_to(&self, id: ConnectionId, message: Message) -> Result<(), SendError> {
        self.0.send_to(id, message)
    }

    /// The current members of `channel` (unordered).
    #[must_use]
    pub fn presence(&self, channel: &str) -> Vec<PresenceMember> {
        self.0.presence(channel)
    }

    /// The number of members currently in `channel`.
    #[must_use]
    pub fn member_count(&self, channel: &str) -> usize {
        self.0.member_count(channel)
    }

    /// Send a message to every connection whose [`ConnectionMeta::key`] is
    /// `key`, locally and on peer instances, returning the number of **local**
    /// connections reached.
    ///
    /// This is the cross-instance form of [`Self::send_to`]: one user can hold
    /// several connections spread over several nodes, and a [`ConnectionId`]
    /// only names one of them on one node. A zero return does not mean the
    /// message went nowhere — a peer may still deliver it.
    pub fn send_to_key(&self, key: &str, message: Message) -> usize {
        self.0.send_to_key(key, message)
    }
}

/// A registered connection handle.
///
/// Dropping the handle deregisters the connection and emits `Leave` events for
/// every channel it was in, so it integrates directly with the `on_upgrade`
/// WebSocket lifecycle.
pub struct Connection {
    hub: Hub,
    id: ConnectionId,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Connection")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// This connection's identifier.
    #[must_use]
    pub const fn id(&self) -> ConnectionId {
        self.id
    }

    /// Join a named channel after passing the [`Authorizer`] hook.
    ///
    /// Joining a channel already joined is a no-op and emits no event.
    ///
    /// # Errors
    ///
    /// Returns [`JoinError::Unauthorized`] if the hook denies the join, or
    /// [`JoinError::ConnectionClosed`] if the connection was already evicted.
    pub fn join(&self, channel: &str) -> Result<(), JoinError> {
        self.hub.0.join(self.id, channel)
    }

    /// Leave a named channel. Leaving a channel not joined is a no-op.
    pub fn leave(&self, channel: &str) {
        self.hub.0.leave(self.id, channel);
    }

    /// Broadcast a message to a channel (convenience for [`Hub::broadcast`]).
    pub fn broadcast(&self, channel: &str, message: Message) {
        self.hub.broadcast(channel, message);
    }

    /// The channels this connection is currently a member of (sorted).
    #[must_use]
    pub fn channels(&self) -> Vec<String> {
        self.hub.0.connection_channels(self.id)
    }

    /// The hub this connection belongs to.
    #[must_use]
    pub fn hub(&self) -> &Hub {
        &self.hub
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.hub.0.disconnect(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Message {
        Message::text(value)
    }

    /// Drain outbound items until the first application message, ignoring
    /// presence events. Returns `None` if the queue drains without one.
    fn next_message(outbound: &mut Outbound) -> Option<Message> {
        while let Some(item) = outbound.try_recv() {
            if let Outgoing::Message(message) = item {
                return Some(message);
            }
        }
        None
    }

    fn drain(outbound: &mut Outbound) {
        while outbound.try_recv().is_some() {}
    }

    fn presence_events(outbound: &mut Outbound) -> Vec<PresenceEvent> {
        let mut events = Vec::new();
        while let Some(item) = outbound.try_recv() {
            if let Outgoing::Presence(event) = item {
                events.push(event);
            }
        }
        events
    }

    #[tokio::test]
    async fn broadcast_reaches_every_subscriber_of_a_channel() {
        let hub = Hub::new();
        let (a, mut a_out) = hub.connect();
        let (b, mut b_out) = hub.connect();
        let (c, mut c_out) = hub.connect();
        a.join("room:1").unwrap();
        b.join("room:1").unwrap();
        c.join("room:1").unwrap();

        hub.broadcast("room:1", text("hello"));

        assert_eq!(next_message(&mut a_out), Some(text("hello")));
        assert_eq!(next_message(&mut b_out), Some(text("hello")));
        assert_eq!(next_message(&mut c_out), Some(text("hello")));
    }

    #[tokio::test]
    async fn leaving_a_channel_stops_further_broadcasts() {
        let hub = Hub::new();
        let (a, mut a_out) = hub.connect();
        let (b, mut b_out) = hub.connect();
        a.join("room:1").unwrap();
        b.join("room:1").unwrap();
        a.leave("room:1");
        drain(&mut a_out);
        drain(&mut b_out);

        hub.broadcast("room:1", text("after-leave"));

        assert_eq!(next_message(&mut a_out), None);
        assert_eq!(next_message(&mut b_out), Some(text("after-leave")));
        assert_eq!(hub.member_count("room:1"), 1);
    }

    #[tokio::test]
    async fn presence_tracks_join_and_leave_events_and_members() {
        let hub = Hub::new();
        let (a, mut a_out) = hub.connect_as(ConnectionMeta::new().with_key("alice"));
        let (b, _b_out) = hub.connect_as(ConnectionMeta::new().with_key("bob"));

        a.join("room:1").unwrap();
        b.join("room:1").unwrap();

        // Alice observes Bob joining.
        let joins = presence_events(&mut a_out);
        assert_eq!(joins.len(), 1);
        assert_eq!(joins[0].kind, PresenceEventKind::Join);
        assert_eq!(joins[0].member.key, "bob");

        let mut keys: Vec<String> = hub.presence("room:1").into_iter().map(|m| m.key).collect();
        keys.sort();
        assert_eq!(keys, vec!["alice".to_owned(), "bob".to_owned()]);

        b.leave("room:1");
        let leaves = presence_events(&mut a_out);
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].kind, PresenceEventKind::Leave);
        assert_eq!(leaves[0].member.key, "bob");

        assert_eq!(hub.member_count("room:1"), 1);
    }

    #[tokio::test]
    async fn dropping_a_connection_emits_leave_to_remaining_members() {
        let hub = Hub::new();
        let (a, mut a_out) = hub.connect_as(ConnectionMeta::new().with_key("alice"));
        let (b, _b_out) = hub.connect_as(ConnectionMeta::new().with_key("bob"));
        a.join("room:1").unwrap();
        b.join("room:1").unwrap();
        drain(&mut a_out);

        drop(b);

        let leaves = presence_events(&mut a_out);
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].kind, PresenceEventKind::Leave);
        assert_eq!(leaves[0].member.key, "bob");
        assert_eq!(hub.member_count("room:1"), 1);
    }

    #[tokio::test]
    async fn directed_send_only_reaches_the_target() {
        let hub = Hub::new();
        let (a, mut a_out) = hub.connect();
        let (b, mut b_out) = hub.connect();

        hub.send_to(a.id(), text("just-you")).unwrap();

        assert_eq!(next_message(&mut a_out), Some(text("just-you")));
        assert_eq!(next_message(&mut b_out), None);
        assert!(matches!(
            hub.send_to(ConnectionId(9999), text("nobody")),
            Err(SendError::UnknownConnection)
        ));
        drop(b);
    }

    #[tokio::test]
    async fn slow_consumer_disconnect_policy_evicts_the_connection() {
        let hub = Hub::builder()
            .capacity(1)
            .slow_consumer(SlowConsumer::Disconnect)
            .build();
        let (a, mut a_out) = hub.connect();
        a.join("room:1").unwrap();

        hub.broadcast("room:1", text("m1")); // fills the queue
        hub.broadcast("room:1", text("m2")); // overflow -> evict

        assert_eq!(hub.member_count("room:1"), 0);
        assert_eq!(a_out.recv().await, Some(Outgoing::Message(text("m1"))));
        assert_eq!(a_out.recv().await, None); // closed after eviction
    }

    #[tokio::test]
    async fn slow_consumer_drop_policy_drops_message_but_keeps_connection() {
        let hub = Hub::builder()
            .capacity(1)
            .slow_consumer(SlowConsumer::DropMessage)
            .build();
        let (a, mut a_out) = hub.connect();
        a.join("room:1").unwrap();

        hub.broadcast("room:1", text("m1")); // fills the queue
        hub.broadcast("room:1", text("m2")); // overflow -> dropped

        assert_eq!(hub.member_count("room:1"), 1); // still connected
        assert_eq!(next_message(&mut a_out), Some(text("m1")));
        assert_eq!(next_message(&mut a_out), None); // m2 was dropped
    }

    #[tokio::test]
    async fn authorizer_denial_prevents_subscription() {
        let hub = Hub::builder()
            .authorizer(|channel: &str, _ctx: &ConnectionContext<'_>| channel != "secret")
            .build();
        let (a, mut a_out) = hub.connect();

        assert!(matches!(a.join("secret"), Err(JoinError::Unauthorized)));
        assert_eq!(a.channels(), Vec::<String>::new());
        assert_eq!(hub.member_count("secret"), 0);

        hub.broadcast("secret", text("blocked"));
        assert_eq!(next_message(&mut a_out), None);

        a.join("public").unwrap();
        assert_eq!(a.channels(), vec!["public".to_owned()]);
    }

    // A test double for the cross-instance seam: shares one broadcast channel
    // and a published log across hubs, standing in for Redis pub/sub.
    #[derive(Clone)]
    struct TestBus {
        published: Arc<Mutex<Vec<PeerFrame>>>,
        sender: tokio::sync::broadcast::Sender<PeerFrame>,
    }

    impl TestBus {
        fn new() -> Self {
            let (sender, _) = tokio::sync::broadcast::channel(64);
            Self {
                published: Arc::new(Mutex::new(Vec::new())),
                sender,
            }
        }

        fn published(&self) -> Vec<PeerFrame> {
            self.published.lock().unwrap().clone()
        }
    }

    impl Broadcaster for TestBus {
        fn publish(&self, frame: &PeerFrame) {
            self.published.lock().unwrap().push(frame.clone());
            let _ = self.sender.send(frame.clone());
        }

        fn subscribe(&self) -> Option<PeerStream> {
            let receiver = self.sender.subscribe();
            let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
                loop {
                    match receiver.recv().await {
                        Ok(frame) => break Some((frame, receiver)),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break None,
                    }
                }
            });
            Some(Box::pin(stream))
        }
    }

    #[tokio::test]
    async fn broadcaster_seam_replicates_across_hubs_without_self_echo() {
        let bus = TestBus::new();
        let hub_a = Hub::builder().broadcaster(bus.clone()).build();
        let hub_b = Hub::builder().broadcaster(bus.clone()).build();

        let (a, mut a_out) = hub_a.connect();
        let (b, mut b_out) = hub_b.connect();
        a.join("room:1").unwrap();
        b.join("room:1").unwrap();

        hub_a.broadcast("room:1", text("cross"));

        // Local delivery on hub A (exactly once, no peer echo).
        assert_eq!(a_out.recv().await, Some(Outgoing::Message(text("cross"))));
        // Peer delivery on hub B via the seam.
        assert_eq!(b_out.recv().await, Some(Outgoing::Message(text("cross"))));

        // publish was invoked exactly once for the local broadcast.
        assert_eq!(bus.published().len(), 1);
        assert_eq!(
            bus.published()[0].target,
            PeerTarget::Channel("room:1".to_owned())
        );

        // Hub A must not double-deliver its own frame.
        assert_eq!(a_out.try_recv(), None);
        drop((a, b));
    }

    #[tokio::test]
    async fn identity_directed_sends_cross_the_seam() {
        let bus = TestBus::new();
        let hub_a = Hub::builder().broadcaster(bus.clone()).build();
        let hub_b = Hub::builder().broadcaster(bus.clone()).build();

        // The same user holds a connection on each instance; a bystander must
        // not receive anything.
        let (here, mut here_out) = hub_a.connect_as(ConnectionMeta::new().with_key("alice"));
        let (elsewhere, mut elsewhere_out) =
            hub_b.connect_as(ConnectionMeta::new().with_key("alice"));
        let (bob, mut bob_out) = hub_b.connect_as(ConnectionMeta::new().with_key("bob"));

        let delivered = hub_a.send_to_key("alice", text("just-alice"));
        assert_eq!(delivered, 1, "only the local connection is counted");

        assert_eq!(
            here_out.recv().await,
            Some(Outgoing::Message(text("just-alice")))
        );
        assert_eq!(
            elsewhere_out.recv().await,
            Some(Outgoing::Message(text("just-alice"))),
            "the peer instance delivers to the same identity"
        );
        assert_eq!(bob_out.try_recv(), None);

        assert_eq!(
            bus.published()[0].target,
            PeerTarget::Key("alice".to_owned())
        );
        // No self-echo on the originating hub.
        assert_eq!(here_out.try_recv(), None);
        drop((here, elsewhere, bob));
    }

    #[tokio::test]
    async fn hub_ids_do_not_collide_within_a_process() {
        let ids: HashSet<u64> = (0..64).map(|_| Hub::new().id().get()).collect();
        assert_eq!(ids.len(), 64, "every hub gets a distinct id");
        assert!(
            !ids.contains(&1),
            "ids are mixed, not a bare counter — two processes starting at 1 \
             would drop each other's frames as self-echoes"
        );
    }
}
