use std::fmt;

use serde::{Deserialize, Serialize};

use crate::PayError;

/// Supported settlement currencies. Only CNY for the first release.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
pub enum Currency {
    /// Chinese yuan, minor unit = fen (1/100 yuan).
    #[default]
    #[serde(rename = "CNY")]
    Cny,
}

impl Currency {
    /// ISO 4217 alphabetic code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cny => "CNY",
        }
    }

    /// Minor units per major unit (fen per yuan).
    #[must_use]
    pub const fn minor_per_major(self) -> u64 {
        match self {
            Self::Cny => 100,
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// Monetary amount stored as an integer count of minor units (分).
///
/// Iron rule: **no floating point anywhere**. Construction, arithmetic, and
/// display all use integer math only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct Amount {
    minor: u64,
    currency: Currency,
}

impl Amount {
    /// Build an amount from minor units of the given currency.
    #[must_use]
    pub const fn from_minor(minor: u64, currency: Currency) -> Self {
        Self { minor, currency }
    }

    /// CNY amount from fen (`Amount::cny(1234)` == ¥12.34).
    #[must_use]
    pub const fn cny(fen: u64) -> Self {
        Self::from_minor(fen, Currency::Cny)
    }

    /// CNY amount from whole yuan plus fen remainder, without floats.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::AmountOverflow`] when `yuan * 100 + fen` exceeds `u64`,
    /// and [`PayError::InvalidOrder`] when `fen >= 100`.
    pub fn cny_yuan_fen(yuan: u64, fen: u8) -> Result<Self, PayError> {
        if fen >= 100 {
            return Err(PayError::InvalidOrder("fen remainder must be < 100"));
        }
        yuan.checked_mul(100)
            .and_then(|base| base.checked_add(u64::from(fen)))
            .map(Self::cny)
            .ok_or(PayError::AmountOverflow)
    }

    /// Total minor units (fen for CNY).
    #[must_use]
    pub const fn minor(self) -> u64 {
        self.minor
    }

    /// The currency of this amount.
    #[must_use]
    pub const fn currency(self) -> Currency {
        self.currency
    }

    /// Whole major units (yuan for CNY), truncated.
    #[must_use]
    pub const fn major_part(self) -> u64 {
        self.minor / self.currency.minor_per_major()
    }

    /// Remaining minor units after [`Self::major_part`].
    #[must_use]
    pub const fn minor_part(self) -> u64 {
        self.minor % self.currency.minor_per_major()
    }

    /// Whether the amount is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.minor == 0
    }

    /// Decimal major-unit string (`10001` fen -> `"100.01"`), integer math
    /// only. This is THE fen -> yuan conversion used by gateway payloads
    /// (e.g. Alipay `total_amount`); do not reimplement it elsewhere.
    #[must_use]
    pub fn decimal_string(self) -> String {
        format!("{}.{:02}", self.major_part(), self.minor_part())
    }

    /// Parse a CNY decimal yuan string (`"100.01"`, `"1"`, `"0.5"`) back into
    /// fen, integer math only. This is THE yuan -> fen conversion for values
    /// coming back from gateways (e.g. Alipay `total_amount` in responses).
    ///
    /// # Errors
    ///
    /// Returns [`PayError::InvalidOrder`] for empty input, non-digit
    /// characters, more than two fraction digits, or a missing integer part,
    /// and [`PayError::AmountOverflow`] when the value exceeds `u64` fen.
    pub fn cny_from_decimal_str(text: &str) -> Result<Self, PayError> {
        let (whole, fraction) = match text.split_once('.') {
            Some((_, "")) => {
                return Err(PayError::InvalidOrder(
                    "amount must not end with a bare decimal point",
                ));
            }
            Some((whole, fraction)) => (whole, fraction),
            None => (text, ""),
        };
        if whole.is_empty() || whole.bytes().any(|byte| !byte.is_ascii_digit()) {
            return Err(PayError::InvalidOrder(
                "amount integer part must be ASCII digits",
            ));
        }
        if fraction.len() > 2 || fraction.bytes().any(|byte| !byte.is_ascii_digit()) {
            return Err(PayError::InvalidOrder(
                "amount fraction must be at most two ASCII digits",
            ));
        }
        let yuan: u64 = whole.parse().map_err(|_| PayError::AmountOverflow)?;
        let mut fen: u64 = 0;
        for byte in fraction.bytes() {
            fen = fen * 10 + u64::from(byte - b'0');
        }
        if fraction.len() == 1 {
            fen *= 10;
        }
        yuan.checked_mul(100)
            .and_then(|base| base.checked_add(fen))
            .map(Self::cny)
            .ok_or(PayError::AmountOverflow)
    }

    /// Checked addition within one currency.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::CurrencyMismatch`] for differing currencies and
    /// [`PayError::AmountOverflow`] on `u64` overflow.
    pub fn checked_add(self, other: Self) -> Result<Self, PayError> {
        if self.currency != other.currency {
            return Err(PayError::CurrencyMismatch {
                left: self.currency.code(),
                right: other.currency.code(),
            });
        }
        self.minor
            .checked_add(other.minor)
            .map(|minor| Self::from_minor(minor, self.currency))
            .ok_or(PayError::AmountOverflow)
    }
}

impl fmt::Display for Amount {
    /// Renders `1234` fen as `12.34 CNY` using integer math only.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}.{:02} {}",
            self.major_part(),
            self.minor_part(),
            self.currency
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_with_integer_math() {
        assert_eq!(Amount::cny(1234).to_string(), "12.34 CNY");
        assert_eq!(Amount::cny(5).to_string(), "0.05 CNY");
        assert_eq!(Amount::cny(100).to_string(), "1.00 CNY");
    }

    #[test]
    fn yuan_fen_constructor_rejects_bad_remainder() {
        assert_eq!(Amount::cny_yuan_fen(12, 34), Ok(Amount::cny(1234)));
        assert_eq!(
            Amount::cny_yuan_fen(1, 100),
            Err(PayError::InvalidOrder("fen remainder must be < 100"))
        );
        assert_eq!(
            Amount::cny_yuan_fen(u64::MAX, 0),
            Err(PayError::AmountOverflow)
        );
    }

    #[test]
    fn decimal_string_uses_integer_math() {
        assert_eq!(Amount::cny(1).decimal_string(), "0.01");
        assert_eq!(Amount::cny(100).decimal_string(), "1.00");
        assert_eq!(Amount::cny(10001).decimal_string(), "100.01");
    }

    #[test]
    fn decimal_str_round_trips() {
        for fen in [1_u64, 100, 10001] {
            let text = Amount::cny(fen).decimal_string();
            assert_eq!(Amount::cny_from_decimal_str(&text), Ok(Amount::cny(fen)));
        }
        assert_eq!(Amount::cny_from_decimal_str("1"), Ok(Amount::cny(100)));
        assert_eq!(Amount::cny_from_decimal_str("0.5"), Ok(Amount::cny(50)));
        for bad in ["", ".", ".5", "1.", "1.234", "1,00", "-1", "1e2", "a"] {
            assert!(
                matches!(
                    Amount::cny_from_decimal_str(bad),
                    Err(PayError::InvalidOrder(_))
                ),
                "`{bad}` must be rejected"
            );
        }
        assert_eq!(
            Amount::cny_from_decimal_str("999999999999999999999"),
            Err(PayError::AmountOverflow)
        );
    }

    #[test]
    fn checked_add_guards_overflow() {
        let sum = Amount::cny(70).checked_add(Amount::cny(30)).unwrap();
        assert_eq!(sum, Amount::cny(100));
        assert_eq!(
            Amount::cny(u64::MAX).checked_add(Amount::cny(1)),
            Err(PayError::AmountOverflow)
        );
    }
}
