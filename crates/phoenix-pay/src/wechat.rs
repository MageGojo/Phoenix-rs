//! `WeChat` Pay `APIv3` protocol pieces: canonical signing strings, the
//! `Authorization` header, platform certificate handling, notify resource
//! decryption, and `trade_state` mapping. Pure functions where possible so
//! everything is unit-testable offline.

use std::collections::HashMap;

use phoenix_http::{HeaderMap, Method};
use serde::Deserialize;

use crate::crypto::{RsaVerifier, aes256_gcm_decrypt, b64_decode};
use crate::{PayError, PaymentStatus, RefundStatus};

/// Production `APIv3` origin. Overridable per provider for tests / mocks.
pub(crate) const DEFAULT_BASE_URL: &str = "https://api.mch.weixin.qq.com";

/// How long downloaded platform certificates are trusted before a refetch.
pub(crate) const CERT_TTL_SECONDS: u64 = 12 * 60 * 60;

/// Accepted clock skew for `Wechatpay-Timestamp` (both directions).
pub(crate) const TIMESTAMP_SKEW_SECONDS: u64 = 300;

/// Canonical request string the merchant private key signs:
/// `{METHOD}\n{path+query}\n{timestamp}\n{nonce}\n{body}\n`.
pub(crate) fn request_message(
    method: &Method,
    path_and_query: &str,
    timestamp: u64,
    nonce: &str,
    body: &str,
) -> String {
    format!("{method}\n{path_and_query}\n{timestamp}\n{nonce}\n{body}\n")
}

/// Canonical response / notify string the platform certificate signed:
/// `{timestamp}\n{nonce}\n{body}\n`. Bytes because the body is raw.
pub(crate) fn response_message(timestamp: &str, nonce: &str, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(timestamp.len() + nonce.len() + body.len() + 3);
    message.extend_from_slice(timestamp.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(nonce.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(body);
    message.push(b'\n');
    message
}

/// `Authorization: WECHATPAY2-SHA256-RSA2048 mchid="...",...` header value.
pub(crate) fn authorization_header(
    mch_id: &str,
    serial_no: &str,
    nonce: &str,
    timestamp: u64,
    signature_base64: &str,
) -> String {
    format!(
        "WECHATPAY2-SHA256-RSA2048 mchid=\"{mch_id}\",nonce_str=\"{nonce}\",\
         signature=\"{signature_base64}\",timestamp=\"{timestamp}\",serial_no=\"{serial_no}\""
    )
}

/// Map `WeChat` `trade_state` to the Phoenix state machine.
pub(crate) fn map_trade_state(trade_state: &str) -> Result<PaymentStatus, PayError> {
    match trade_state {
        "SUCCESS" => Ok(PaymentStatus::Paid),
        "NOTPAY" | "USERPAYING" | "ACCEPT" => Ok(PaymentStatus::Pending),
        "CLOSED" | "REVOKED" => Ok(PaymentStatus::Closed),
        "PAYERROR" => Ok(PaymentStatus::Failed),
        "REFUND" => Ok(PaymentStatus::Refunding),
        other => Err(PayError::InvalidNotify(format!(
            "unknown WeChat trade_state `{other}`"
        ))),
    }
}

/// The four `Wechatpay-*` signature headers of a response / notification.
pub(crate) struct SignatureHeaders {
    pub timestamp: String,
    pub nonce: String,
    pub signature: Vec<u8>,
    pub serial: String,
}

impl SignatureHeaders {
    /// Extract and base64-decode the signature headers.
    pub(crate) fn from_headers(headers: &HeaderMap) -> Result<Self, PayError> {
        let get = |name: &str| -> Result<String, PayError> {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
                .ok_or_else(|| {
                    PayError::InvalidNotify(format!("missing or non-ASCII `{name}` header"))
                })
        };
        let signature = b64_decode(&get("Wechatpay-Signature")?)
            .map_err(|error| PayError::InvalidNotify(format!("Wechatpay-Signature: {error}")))?;
        Ok(Self {
            timestamp: get("Wechatpay-Timestamp")?,
            nonce: get("Wechatpay-Nonce")?,
            signature,
            serial: get("Wechatpay-Serial")?,
        })
    }

    /// Reject timestamps outside the accepted skew window (replay guard).
    pub(crate) fn check_freshness(&self, now: u64) -> Result<(), PayError> {
        let timestamp: u64 = self.timestamp.parse().map_err(|_| {
            PayError::InvalidNotify(format!(
                "non-numeric Wechatpay-Timestamp `{}`",
                self.timestamp
            ))
        })?;
        if now.abs_diff(timestamp) > TIMESTAMP_SKEW_SECONDS {
            return Err(PayError::InvalidNotify(format!(
                "Wechatpay-Timestamp {timestamp} outside the ±{TIMESTAMP_SKEW_SECONDS}s window"
            )));
        }
        Ok(())
    }
}

/// In-process platform certificate cache, keyed by uppercase serial.
pub(crate) struct PlatformCerts {
    verifiers: HashMap<String, RsaVerifier>,
    fetched_at: u64,
    /// File-loaded certificates never expire (no way to refetch them).
    from_file: bool,
}

impl PlatformCerts {
    pub(crate) fn new(
        verifiers: HashMap<String, RsaVerifier>,
        fetched_at: u64,
        from_file: bool,
    ) -> Self {
        Self {
            verifiers,
            fetched_at,
            from_file,
        }
    }

    /// Case-insensitive serial lookup.
    pub(crate) fn get(&self, serial: &str) -> Option<&RsaVerifier> {
        self.verifiers.get(&serial.to_ascii_uppercase())
    }

    /// Whether the cache is still inside [`CERT_TTL_SECONDS`].
    pub(crate) fn is_fresh(&self, now: u64) -> bool {
        self.from_file || now.saturating_sub(self.fetched_at) <= CERT_TTL_SECONDS
    }
}

/// `WeChat` encrypted resource envelope (`AEAD_AES_256_GCM`).
#[derive(Debug, Deserialize)]
pub(crate) struct EncryptedResource {
    #[serde(default)]
    pub algorithm: String,
    pub ciphertext: String,
    pub nonce: String,
    #[serde(default)]
    pub associated_data: String,
}

impl EncryptedResource {
    /// Decrypt with the `APIv3` key. Errors are plain strings; callers pick
    /// the [`PayError`] variant fitting their context.
    pub(crate) fn decrypt(&self, api_v3_key: &[u8]) -> Result<Vec<u8>, String> {
        if !self.algorithm.is_empty() && self.algorithm != "AEAD_AES_256_GCM" {
            return Err(format!(
                "unsupported resource algorithm `{}`",
                self.algorithm
            ));
        }
        let ciphertext = b64_decode(&self.ciphertext)?;
        aes256_gcm_decrypt(
            api_v3_key,
            self.nonce.as_bytes(),
            self.associated_data.as_bytes(),
            &ciphertext,
        )
    }
}

/// Body of a payment notification (before resource decryption).
#[derive(Debug, Deserialize)]
pub(crate) struct NotifyBody {
    pub resource: EncryptedResource,
}

/// Decrypted transaction resource (payment result).
#[derive(Debug, Deserialize)]
pub(crate) struct TransactionResource {
    pub out_trade_no: String,
    #[serde(default)]
    pub transaction_id: Option<String>,
    pub trade_state: String,
}

/// Map `WeChat` refund `status` to the refund state machine.
///
/// `ABNORMAL` means the refund could not be completed automatically and needs
/// manual handling in the merchant console — the money has *not* moved, so it
/// maps to `Failed` rather than being left pending forever.
pub(crate) fn map_refund_status(status: &str) -> Result<RefundStatus, PayError> {
    match status {
        "SUCCESS" => Ok(RefundStatus::Succeeded),
        "PROCESSING" => Ok(RefundStatus::Processing),
        "CLOSED" | "ABNORMAL" => Ok(RefundStatus::Failed),
        other => Err(PayError::Gateway(format!(
            "unknown WeChat refund status `{other}`"
        ))),
    }
}

/// Refund response / query resource.
#[derive(Debug, Deserialize)]
pub(crate) struct RefundResource {
    #[serde(default)]
    pub out_trade_no: String,
    pub out_refund_no: String,
    #[serde(default)]
    pub refund_id: Option<String>,
    pub status: String,
    pub amount: RefundAmount,
}

/// The `amount` block of a refund resource (minor units).
#[derive(Debug, Deserialize)]
pub(crate) struct RefundAmount {
    pub refund: u64,
}

/// `GET /v3/certificates` response body.
#[derive(Debug, Deserialize)]
pub(crate) struct CertificatesBody {
    pub data: Vec<CertificateEntry>,
}

/// One platform certificate entry.
#[derive(Debug, Deserialize)]
pub(crate) struct CertificateEntry {
    pub serial_no: String,
    pub encrypt_certificate: EncryptedResource,
}

/// Error body (`{"code":"...","message":"..."}`) of a non-2xx response.
#[derive(Debug, Deserialize)]
pub(crate) struct ErrorBody {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_strings_match_the_spec() {
        assert_eq!(
            request_message(
                &Method::POST,
                "/v3/pay/transactions/native",
                1_700_000_000,
                "N1",
                "{}"
            ),
            "POST\n/v3/pay/transactions/native\n1700000000\nN1\n{}\n"
        );
        assert_eq!(
            request_message(
                &Method::GET,
                "/v3/pay/transactions/out-trade-no/T1?mchid=m",
                1,
                "N",
                ""
            ),
            "GET\n/v3/pay/transactions/out-trade-no/T1?mchid=m\n1\nN\n\n"
        );
        assert_eq!(
            response_message("1700000000", "N1", b"{\"a\":1}"),
            b"1700000000\nN1\n{\"a\":1}\n"
        );
        let header = authorization_header("m1", "S1", "N1", 42, "sig==");
        assert!(header.starts_with("WECHATPAY2-SHA256-RSA2048 mchid=\"m1\","));
        assert!(header.contains("nonce_str=\"N1\""));
        assert!(header.contains("signature=\"sig==\""));
        assert!(header.contains("timestamp=\"42\""));
        assert!(header.ends_with("serial_no=\"S1\""));
    }

    #[test]
    fn trade_states_map_to_the_state_machine() {
        assert_eq!(map_trade_state("SUCCESS"), Ok(PaymentStatus::Paid));
        assert_eq!(map_trade_state("NOTPAY"), Ok(PaymentStatus::Pending));
        assert_eq!(map_trade_state("USERPAYING"), Ok(PaymentStatus::Pending));
        assert_eq!(map_trade_state("CLOSED"), Ok(PaymentStatus::Closed));
        assert_eq!(map_trade_state("REVOKED"), Ok(PaymentStatus::Closed));
        assert_eq!(map_trade_state("PAYERROR"), Ok(PaymentStatus::Failed));
        assert_eq!(map_trade_state("REFUND"), Ok(PaymentStatus::Refunding));
        assert!(map_trade_state("HACKED").is_err());
    }

    #[test]
    fn signature_headers_freshness_window() {
        let mut headers = HeaderMap::new();
        headers.insert("Wechatpay-Timestamp", "1000".parse().unwrap());
        headers.insert("Wechatpay-Nonce", "N".parse().unwrap());
        headers.insert("Wechatpay-Signature", "c2ln".parse().unwrap());
        headers.insert("Wechatpay-Serial", "S".parse().unwrap());
        let parsed = SignatureHeaders::from_headers(&headers).expect("headers");
        assert_eq!(parsed.signature, b"sig");
        assert!(parsed.check_freshness(1000).is_ok());
        assert!(parsed.check_freshness(1300).is_ok());
        assert!(parsed.check_freshness(1301).is_err());
        assert!(parsed.check_freshness(700).is_ok());
        assert!(parsed.check_freshness(699).is_err());

        headers.remove("Wechatpay-Serial");
        assert!(SignatureHeaders::from_headers(&headers).is_err());
    }
}
