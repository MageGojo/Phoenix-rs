//! Job identity and envelope types.

use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a queued job.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(Uuid);

impl JobId {
    /// Generate a new random job id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Borrow the underlying UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Debug for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("JobId")
            .field(&self.0.to_string())
            .finish()
    }
}

/// Serializable job record stored by a [`crate::QueueBackend`].
#[derive(Clone, Serialize, Deserialize)]
pub struct JobEnvelope {
    /// Stable job identity.
    pub id: JobId,
    /// Handler routing name (e.g. `"send-welcome-email"`).
    pub name: String,
    /// Opaque JSON payload. Redacted in [`Debug`].
    pub payload: serde_json::Value,
    /// How many times this job has been reserved for execution.
    pub attempts: u32,
    /// Maximum reserves before dead-lettering.
    pub max_attempts: u32,
    /// Optional dedupe key while the job is still in-flight.
    pub idempotency_key: Option<String>,
    /// Earliest time the job may be reserved.
    #[serde(with = "system_time_secs")]
    pub available_at: SystemTime,
    /// Creation timestamp.
    #[serde(with = "system_time_secs")]
    pub created_at: SystemTime,
}

impl JobEnvelope {
    /// Build a new envelope ready to push.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        payload: serde_json::Value,
        max_attempts: u32,
        idempotency_key: Option<String>,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            id: JobId::new(),
            name: name.into(),
            payload,
            attempts: 0,
            max_attempts: max_attempts.max(1),
            idempotency_key,
            available_at: now,
            created_at: now,
        }
    }

    /// Whether another attempt is allowed after the current failure.
    #[must_use]
    pub const fn can_retry(&self) -> bool {
        self.attempts < self.max_attempts
    }

    /// Delay until `available_at` relative to `now`, if still in the future.
    #[must_use]
    pub fn delay_until_available(&self, now: SystemTime) -> Option<Duration> {
        self.available_at.duration_since(now).ok()
    }
}

impl fmt::Debug for JobEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JobEnvelope")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("payload", &"<redacted>")
            .field("attempts", &self.attempts)
            .field("max_attempts", &self.max_attempts)
            .field("idempotency_key", &self.idempotency_key)
            .field("available_at", &format_system_time(self.available_at))
            .field("created_at", &format_system_time(self.created_at))
            .finish()
    }
}

fn format_system_time(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("{}s", duration.as_secs()),
        Err(_) => "<before-unix-epoch>".to_owned(),
    }
}

mod system_time_secs {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let secs = time
            .duration_since(UNIX_EPOCH)
            .map_err(serde::ser::Error::custom)?
            .as_secs();
        serializer.serialize_u64(secs)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_payload() {
        let job = JobEnvelope::new("demo", serde_json::json!({"secret": "value"}), 3, None);
        let rendered = format!("{job:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("value"));
    }
}
