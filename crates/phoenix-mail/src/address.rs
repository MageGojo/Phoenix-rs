use std::{fmt, str::FromStr};

use crate::MailError;

/// A validated mailbox address for outbound mail headers.
///
/// Validation is intentionally basic for the first release:
/// - non-empty after trim
/// - contains `@`
/// - no CR, LF, or other ASCII control characters
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Address {
    value: String,
}

impl Address {
    /// Parse and validate a mailbox address.
    ///
    /// # Errors
    ///
    /// Returns [`MailError::InvalidAddress`] when the value fails basic checks.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, MailError> {
        let raw = value.as_ref().trim();
        if raw.is_empty() {
            return Err(MailError::InvalidAddress {
                reason: "address is empty",
            });
        }
        if contains_control_char(raw) {
            return Err(MailError::InvalidAddress {
                reason: "address contains control characters",
            });
        }
        if !raw.contains('@') {
            return Err(MailError::InvalidAddress {
                reason: "address must contain '@'",
            });
        }
        Ok(Self {
            value: raw.to_owned(),
        })
    }

    /// Borrow the validated address string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl AsRef<str> for Address {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Address {
    type Err = MailError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for Address {
    type Error = MailError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for Address {
    type Error = MailError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// True when `value` contains CR, LF, or any other ASCII control character.
pub(crate) fn contains_control_char(value: &str) -> bool {
    value.chars().any(char::is_control)
}

/// Reject CR/LF (and other controls) in header fields such as `Subject`.
pub(crate) fn reject_header_injection(field: &'static str, value: &str) -> Result<(), MailError> {
    if contains_control_char(value) {
        return Err(MailError::HeaderInjection { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_mailbox() {
        let address = Address::parse("user@example.com").expect("valid");
        assert_eq!(address.as_str(), "user@example.com");
    }

    #[test]
    fn rejects_empty_and_missing_at() {
        assert!(matches!(
            Address::parse("   "),
            Err(MailError::InvalidAddress { .. })
        ));
        assert!(matches!(
            Address::parse("not-an-email"),
            Err(MailError::InvalidAddress { .. })
        ));
    }

    #[test]
    fn rejects_control_characters() {
        assert!(matches!(
            Address::parse("user\r@example.com"),
            Err(MailError::InvalidAddress { .. })
        ));
        assert!(matches!(
            Address::parse("user\n@example.com"),
            Err(MailError::InvalidAddress { .. })
        ));
    }
}
