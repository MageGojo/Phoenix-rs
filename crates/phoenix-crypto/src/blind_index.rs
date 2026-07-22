use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

const MIN_SECRET_BYTES: usize = 32;
const MAX_KEY_ID_BYTES: usize = 64;
const MAX_PURPOSE_BYTES: usize = 128;
const MAX_ENVELOPE_BYTES: usize = 192;
const FRAME_LABEL: &[u8] = b"phoenix.blind-index";
const FRAME_VERSION: u8 = 1;
const ENVELOPE_PREFIX: &str = "phxbi";
const ENVELOPE_VERSION: &str = "v1";
const ENVELOPE_ALGORITHM: &str = "hmac-sha256";
const TAG_BYTES: usize = 32;

/// Maximum number of active and legacy keys in one blind-index key ring.
pub const MAX_BLIND_INDEX_KEYS: usize = 8;

type HmacSha256 = Hmac<Sha256>;

/// A dedicated HMAC key for deterministic blind indexes.
///
/// This key must not be reused for encryption, JWTs, sessions, or any other
/// cryptographic purpose.
#[derive(Clone)]
pub struct BlindIndexKey {
    id: String,
    secret: Zeroizing<Vec<u8>>,
}

impl BlindIndexKey {
    /// Construct a blind-index key with a stable rotation ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the ID is empty or too long, or when the secret is
    /// shorter than 32 bytes.
    pub fn new(id: impl Into<String>, secret: impl AsRef<[u8]>) -> Result<Self, BlindIndexError> {
        let id = id.into();
        validate_key_id(&id)?;
        if secret.as_ref().len() < MIN_SECRET_BYTES {
            return Err(BlindIndexError::WeakKey);
        }
        Ok(Self {
            id,
            secret: Zeroizing::new(secret.as_ref().to_vec()),
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl std::fmt::Debug for BlindIndexKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BlindIndexKey")
            .field("id", &self.id)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

/// Versioned HMAC-SHA256 blind indexes with bounded key rotation.
///
/// Blind indexes provide deterministic equality lookup, not encryption. The
/// caller is responsible for normalizing values before indexing them.
#[derive(Clone)]
pub struct BlindIndexer {
    active: BlindIndexKey,
    verification_keys: Vec<BlindIndexKey>,
}

impl BlindIndexer {
    #[must_use]
    pub const fn new(active: BlindIndexKey) -> Self {
        Self {
            active,
            verification_keys: Vec::new(),
        }
    }

    /// Add a legacy key used for verification and rotation query candidates.
    ///
    /// Legacy keys never become active. Candidate order is the active key
    /// followed by legacy keys in the order they were added.
    ///
    /// # Errors
    ///
    /// Returns an error for a duplicate key ID or when the bounded key-ring
    /// capacity would be exceeded.
    pub fn with_verification_key(mut self, key: BlindIndexKey) -> Result<Self, BlindIndexError> {
        if self.key_by_id(key.id()).is_some() {
            return Err(BlindIndexError::DuplicateKeyId);
        }
        if self.key_count() >= MAX_BLIND_INDEX_KEYS {
            return Err(BlindIndexError::TooManyKeys);
        }
        self.verification_keys.push(key);
        Ok(self)
    }

    #[must_use]
    pub fn active_key_id(&self) -> &str {
        self.active.id()
    }

    #[must_use]
    pub const fn key_count(&self) -> usize {
        1 + self.verification_keys.len()
    }

    /// Generate an envelope using the active key.
    ///
    /// # Errors
    ///
    /// Returns an error when the purpose is empty or too long, the value cannot
    /// be framed, or HMAC initialization fails.
    pub fn index(&self, purpose: &str, value: &[u8]) -> Result<String, BlindIndexError> {
        validate_purpose(purpose)?;
        index_with_key(&self.active, purpose, value)
    }

    /// Generate bounded equality-query candidates for the active and legacy keys.
    ///
    /// # Errors
    ///
    /// Returns an error when the purpose or value cannot be indexed.
    pub fn candidates(&self, purpose: &str, value: &[u8]) -> Result<Vec<String>, BlindIndexError> {
        validate_purpose(purpose)?;
        self.keys()
            .map(|key| index_with_key(key, purpose, value))
            .collect()
    }

    /// Verify an envelope using its declared key ID and a constant-time tag check.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid purposes, malformed envelopes, unknown key
    /// IDs, or tags that do not authenticate the purpose and value.
    pub fn verify(
        &self,
        encoded: &str,
        purpose: &str,
        value: &[u8],
    ) -> Result<(), BlindIndexError> {
        validate_purpose(purpose)?;
        let parsed = parse_envelope(encoded)?;
        let key = self
            .key_by_id(&parsed.key_id)
            .ok_or(BlindIndexError::UnknownKey)?;
        let mac = framed_mac(key, purpose, value)?;
        mac.verify_slice(&parsed.tag)
            .map_err(|_| BlindIndexError::VerificationFailed)
    }

    fn keys(&self) -> impl Iterator<Item = &BlindIndexKey> {
        std::iter::once(&self.active).chain(self.verification_keys.iter())
    }

    fn key_by_id(&self, id: &str) -> Option<&BlindIndexKey> {
        self.keys().find(|key| key.id() == id)
    }
}

impl std::fmt::Debug for BlindIndexer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BlindIndexer")
            .field("active_key_id", &self.active.id())
            .field(
                "verification_key_ids",
                &self
                    .verification_keys
                    .iter()
                    .map(BlindIndexKey::id)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum BlindIndexError {
    #[error("blind-index key IDs must contain between 1 and 64 bytes")]
    InvalidKeyId,
    #[error("blind-index HMAC keys must contain at least 32 bytes")]
    WeakKey,
    #[error("blind-index key IDs must be unique within a key ring")]
    DuplicateKeyId,
    #[error("blind-index key rings cannot contain more than 8 keys")]
    TooManyKeys,
    #[error("blind-index purposes must contain between 1 and 128 bytes")]
    InvalidPurpose,
    #[error("blind-index values are too long to frame")]
    ValueTooLong,
    #[error("blind-index HMAC initialization failed")]
    HmacInitialization,
    #[error("blind-index data has an unsupported or malformed envelope")]
    InvalidEnvelope,
    #[error("blind-index key ID is not recognized")]
    UnknownKey,
    #[error("blind-index verification failed")]
    VerificationFailed,
}

struct ParsedEnvelope {
    key_id: String,
    tag: [u8; TAG_BYTES],
}

fn validate_key_id(id: &str) -> Result<(), BlindIndexError> {
    if id.trim().is_empty() || id.len() > MAX_KEY_ID_BYTES || id.chars().any(char::is_control) {
        return Err(BlindIndexError::InvalidKeyId);
    }
    Ok(())
}

fn validate_purpose(purpose: &str) -> Result<(), BlindIndexError> {
    if purpose.trim().is_empty()
        || purpose.len() > MAX_PURPOSE_BYTES
        || purpose.chars().any(char::is_control)
    {
        return Err(BlindIndexError::InvalidPurpose);
    }
    Ok(())
}

fn index_with_key(
    key: &BlindIndexKey,
    purpose: &str,
    value: &[u8],
) -> Result<String, BlindIndexError> {
    let tag = framed_mac(key, purpose, value)?.finalize().into_bytes();
    Ok(format!(
        "{ENVELOPE_PREFIX}.{ENVELOPE_VERSION}.{ENVELOPE_ALGORITHM}.{}.{}",
        URL_SAFE_NO_PAD.encode(key.id.as_bytes()),
        URL_SAFE_NO_PAD.encode(tag)
    ))
}

fn framed_mac(
    key: &BlindIndexKey,
    purpose: &str,
    value: &[u8],
) -> Result<HmacSha256, BlindIndexError> {
    let key_id_length = u16::try_from(key.id.len()).map_err(|_| BlindIndexError::InvalidKeyId)?;
    let purpose_length =
        u16::try_from(purpose.len()).map_err(|_| BlindIndexError::InvalidPurpose)?;
    let value_length = u64::try_from(value.len()).map_err(|_| BlindIndexError::ValueTooLong)?;
    let mut mac = HmacSha256::new_from_slice(key.secret.as_ref())
        .map_err(|_| BlindIndexError::HmacInitialization)?;
    mac.update(FRAME_LABEL);
    mac.update(&[FRAME_VERSION]);
    mac.update(&key_id_length.to_be_bytes());
    mac.update(key.id.as_bytes());
    mac.update(&purpose_length.to_be_bytes());
    mac.update(purpose.as_bytes());
    mac.update(&value_length.to_be_bytes());
    mac.update(value);
    Ok(mac)
}

fn parse_envelope(encoded: &str) -> Result<ParsedEnvelope, BlindIndexError> {
    if encoded.len() > MAX_ENVELOPE_BYTES || encoded.chars().any(char::is_whitespace) {
        return Err(BlindIndexError::InvalidEnvelope);
    }
    let mut segments = encoded.split('.');
    let (Some(prefix), Some(version), Some(algorithm), Some(key_id), Some(tag)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(BlindIndexError::InvalidEnvelope);
    };
    if segments.next().is_some()
        || prefix != ENVELOPE_PREFIX
        || version != ENVELOPE_VERSION
        || algorithm != ENVELOPE_ALGORITHM
    {
        return Err(BlindIndexError::InvalidEnvelope);
    }

    let key_id_bytes = decode_canonical(key_id)?;
    let key_id = String::from_utf8(key_id_bytes).map_err(|_| BlindIndexError::InvalidEnvelope)?;
    validate_key_id(&key_id).map_err(|_| BlindIndexError::InvalidEnvelope)?;
    let tag_bytes = decode_canonical(tag)?;
    let tag = tag_bytes
        .try_into()
        .map_err(|_| BlindIndexError::InvalidEnvelope)?;
    Ok(ParsedEnvelope { key_id, tag })
}

fn decode_canonical(encoded: &str) -> Result<Vec<u8>, BlindIndexError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| BlindIndexError::InvalidEnvelope)?;
    if URL_SAFE_NO_PAD.encode(&decoded) != encoded {
        return Err(BlindIndexError::InvalidEnvelope);
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str, byte: u8) -> BlindIndexKey {
        BlindIndexKey::new(id, [byte; 32]).unwrap()
    }

    #[test]
    fn indexes_stably_and_isolates_purposes() {
        let indexer = BlindIndexer::new(key("active", 7));
        let first = indexer.index("APP_A|devices|udid", b"device-1").unwrap();
        let second = indexer.index("APP_A|devices|udid", b"device-1").unwrap();
        let other_purpose = indexer
            .index("APP_A|users|external-id", b"device-1")
            .unwrap();

        assert_eq!(first, second);
        assert_ne!(first, other_purpose);
        assert_eq!(
            first,
            "phxbi.v1.hmac-sha256.YWN0aXZl.M9MyU5OZxxl8PJ-C9uw-hBVhRNq9rLuA-9WSw8kGXh0"
        );
        assert!(
            indexer
                .verify(&first, "APP_A|devices|udid", b"device-1")
                .is_ok()
        );
        assert_eq!(
            indexer.verify(&first, "APP_A|users|external-id", b"device-1"),
            Err(BlindIndexError::VerificationFailed)
        );
    }

    #[test]
    fn different_keys_produce_different_indexes() {
        let first = BlindIndexer::new(key("active", 1))
            .index("APP_A|devices|udid", b"device-1")
            .unwrap();
        let second_indexer = BlindIndexer::new(key("active", 2));
        let second = second_indexer
            .index("APP_A|devices|udid", b"device-1")
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(
            second_indexer.verify(&first, "APP_A|devices|udid", b"device-1"),
            Err(BlindIndexError::VerificationFailed)
        );
    }

    #[test]
    fn rotation_verifies_old_indexes_and_generates_bounded_candidates() {
        let old_indexer = BlindIndexer::new(key("old-1", 11));
        let old_index = old_indexer
            .index("APP_A|devices|udid", b"device-1")
            .unwrap();
        let indexer = BlindIndexer::new(key("current", 13))
            .with_verification_key(key("old-1", 11))
            .unwrap()
            .with_verification_key(key("old-2", 12))
            .unwrap();
        let candidates = indexer
            .candidates("APP_A|devices|udid", b"device-1")
            .unwrap();

        assert_eq!(indexer.active_key_id(), "current");
        assert_eq!(indexer.key_count(), 3);
        assert_eq!(candidates.len(), 3);
        assert_eq!(
            candidates[0],
            indexer.index("APP_A|devices|udid", b"device-1").unwrap()
        );
        assert_eq!(candidates[1], old_index);
        assert!(candidates.iter().all(|candidate| {
            indexer
                .verify(candidate, "APP_A|devices|udid", b"device-1")
                .is_ok()
        }));
        assert!(
            indexer
                .verify(&old_index, "APP_A|devices|udid", b"device-1")
                .is_ok()
        );
    }

    #[test]
    fn rejects_weak_or_invalid_keys_and_unbounded_rings() {
        assert!(matches!(
            BlindIndexKey::new("active", [1_u8; 31]),
            Err(BlindIndexError::WeakKey)
        ));
        assert!(matches!(
            BlindIndexKey::new(" ", [1_u8; 32]),
            Err(BlindIndexError::InvalidKeyId)
        ));
        assert!(matches!(
            BlindIndexKey::new("x".repeat(MAX_KEY_ID_BYTES + 1), [1_u8; 32]),
            Err(BlindIndexError::InvalidKeyId)
        ));
        assert!(matches!(
            BlindIndexer::new(key("active", 1)).with_verification_key(key("active", 2)),
            Err(BlindIndexError::DuplicateKeyId)
        ));

        let mut indexer = BlindIndexer::new(key("key-0", 0));
        for byte in 1..MAX_BLIND_INDEX_KEYS {
            indexer = indexer
                .with_verification_key(key(&format!("key-{byte}"), u8::try_from(byte).unwrap()))
                .unwrap();
        }
        assert!(matches!(
            indexer.with_verification_key(key("one-too-many", 9)),
            Err(BlindIndexError::TooManyKeys)
        ));
    }

    #[test]
    fn rejects_invalid_purposes() {
        let indexer = BlindIndexer::new(key("active", 7));
        assert_eq!(
            indexer.index(" ", b"device-1"),
            Err(BlindIndexError::InvalidPurpose)
        );
        assert_eq!(
            indexer.index(&"x".repeat(MAX_PURPOSE_BYTES + 1), b"device-1"),
            Err(BlindIndexError::InvalidPurpose)
        );
        assert_eq!(
            indexer.index("APP_A\ndevices", b"device-1"),
            Err(BlindIndexError::InvalidPurpose)
        );
    }

    #[test]
    fn debug_output_never_contains_key_material() {
        let secret = b"blind-index-secret-that-must-never-appear";
        let key = BlindIndexKey::new("active", secret).unwrap();
        let key_debug = format!("{key:?}");
        let indexer_debug = format!("{:?}", BlindIndexer::new(key));

        assert!(key_debug.contains("[REDACTED]"));
        assert!(!key_debug.contains(std::str::from_utf8(secret).unwrap()));
        assert!(!indexer_debug.contains(std::str::from_utf8(secret).unwrap()));
    }

    #[test]
    fn malformed_unknown_and_unauthenticated_envelopes_fail_closed() {
        let indexer = BlindIndexer::new(key("active", 7));
        let valid = indexer.index("APP_A|devices|udid", b"device-1").unwrap();
        let malformed = [
            "",
            "phxbi.v1.hmac-sha256",
            "phxbi.v2.hmac-sha256.YWN0aXZl.AA",
            "phxbi.v1.sha256.YWN0aXZl.AA",
            "phxbi.v1.hmac-sha256.***.AA",
            "phxbi.v1.hmac-sha256.YWN0aXZl.AA",
            "phxbi.v1.hmac-sha256.YWN0aXZl=.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "phxbi.v1.hmac-sha256.YWN0aXZl.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA.extra",
        ];
        for encoded in malformed {
            assert_eq!(
                indexer.verify(encoded, "APP_A|devices|udid", b"device-1"),
                Err(BlindIndexError::InvalidEnvelope),
                "accepted malformed envelope: {encoded}"
            );
        }

        let unknown = valid.replace(".YWN0aXZl.", ".bWlzc2luZw.");
        assert_eq!(
            indexer.verify(&unknown, "APP_A|devices|udid", b"device-1"),
            Err(BlindIndexError::UnknownKey)
        );
        assert_eq!(
            indexer.verify(&valid, "APP_A|devices|udid", b"device-2"),
            Err(BlindIndexError::VerificationFailed)
        );
    }
}
