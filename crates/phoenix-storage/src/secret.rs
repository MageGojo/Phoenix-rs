//! A redacted, zeroized-on-drop secret wrapper for storage credentials.
//!
//! Mirrors the `Secret` handling used elsewhere in the repository (see
//! `phoenix_pay::prelude::Secret`): the value is zeroized when dropped and its
//! `Debug` never leaks the contents, so an [`crate::S3Config`] can be logged
//! without exposing the S3/OSS secret access key.

use std::fmt;

use zeroize::Zeroizing;

/// Secret credential value: zeroized on drop, redacted in `Debug`.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// Wrap a secret string (for example an S3 secret access key).
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Borrow the secret. Never log or `Debug`-print the result.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret([REDACTED])")
    }
}

impl<T: Into<String>> From<T> for Secret {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn debug_is_redacted_but_expose_works() {
        let secret = Secret::new("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");
        assert_eq!(secret.expose(), "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");
        let debug = format!("{secret:?}");
        assert_eq!(debug, "Secret([REDACTED])");
        assert!(!debug.contains("wJal"));
    }

    #[test]
    fn from_str_wraps() {
        let secret: Secret = "abc".into();
        assert_eq!(secret.expose(), "abc");
    }
}
