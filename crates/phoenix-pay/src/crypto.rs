//! Offline cryptography for the real gateways: RSA-SHA256 (PKCS#1 v1.5)
//! signing / verification via `ring`, AES-256-GCM decryption via `aes-gcm`,
//! PEM / DER key material loading, and small time / encoding helpers.
//!
//! Everything in this module is pure (no network); unit tests prove sign ->
//! verify loopback with the fixtures under `tests/fixtures/` and that any
//! tampering fails verification.

use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{
    RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_SHA256, RsaKeyPair, UnparsedPublicKey,
};
use x509_parser::prelude::FromDer;
use x509_parser::x509::SubjectPublicKeyInfo;

use crate::PayError;

/// Base64 (standard alphabet, padded) encode.
pub(crate) fn b64_encode(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

/// Base64 (standard alphabet, padded) decode.
pub(crate) fn b64_decode(text: &str) -> Result<Vec<u8>, String> {
    BASE64
        .decode(text.trim())
        .map_err(|error| format!("invalid base64: {error}"))
}

/// Uppercase hex, the format `WeChat` uses for certificate serial numbers.
pub(crate) fn hex_upper(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out.to_ascii_uppercase()
}

/// Current unix timestamp in seconds.
pub(crate) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Cryptographically random nonce string (32 hex chars).
pub(crate) fn random_nonce() -> Result<String, PayError> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| PayError::Config("system RNG failure while generating a nonce".to_owned()))?;
    Ok(hex_upper(&bytes))
}

/// `yyyy-MM-dd HH:mm:ss` in GMT+8 (Alipay's required timestamp format),
/// integer math only (Howard Hinnant's `civil_from_days`).
pub(crate) fn gmt8_datetime(unix_seconds: u64) -> String {
    let local = unix_seconds + 8 * 3600;
    let days = local / 86_400;
    let secs = local % 86_400;
    // civil_from_days, shifted so the era math stays in u64 (unix >= 0).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// RSA-SHA256 (PKCS#1 v1.5) signer over a merchant / application private key.
pub(crate) struct RsaSigner {
    key_pair: RsaKeyPair,
    rng: SystemRandom,
}

impl std::fmt::Debug for RsaSigner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RsaSigner([REDACTED])")
    }
}

impl RsaSigner {
    /// Load from a PEM (`PRIVATE KEY` PKCS#8 or `RSA PRIVATE KEY` PKCS#1)
    /// document, or from the bare base64 DER body Alipay consoles hand out.
    pub(crate) fn from_pem(pem: &str) -> Result<Self, PayError> {
        let mut reader = pem.as_bytes();
        for item in rustls_pemfile::read_all(&mut reader).flatten() {
            let key_pair = match &item {
                rustls_pemfile::Item::Pkcs8Key(der) => {
                    RsaKeyPair::from_pkcs8(der.secret_pkcs8_der())
                }
                rustls_pemfile::Item::Pkcs1Key(der) => RsaKeyPair::from_der(der.secret_pkcs1_der()),
                _ => continue,
            };
            return key_pair
                .map(Self::new)
                .map_err(|error| PayError::Config(format!("unusable RSA private key: {error}")));
        }
        // No PEM armor: treat the input as bare base64 DER (PKCS#8 or PKCS#1).
        let compact: String = pem.split_whitespace().collect();
        let der = b64_decode(&compact).map_err(|_| {
            PayError::Config("private key is neither PEM nor base64 DER".to_owned())
        })?;
        RsaKeyPair::from_pkcs8(&der)
            .or_else(|_| RsaKeyPair::from_der(&der))
            .map(Self::new)
            .map_err(|error| PayError::Config(format!("unusable RSA private key: {error}")))
    }

    fn new(key_pair: RsaKeyPair) -> Self {
        Self {
            key_pair,
            rng: SystemRandom::new(),
        }
    }

    /// Sign `message` with RSA-SHA256 (PKCS#1 v1.5).
    pub(crate) fn sign(&self, message: &[u8]) -> Result<Vec<u8>, PayError> {
        let mut signature = vec![0_u8; self.key_pair.public().modulus_len()];
        self.key_pair
            .sign(&RSA_PKCS1_SHA256, &self.rng, message, &mut signature)
            .map_err(|_| PayError::Config("RSA signing failed".to_owned()))?;
        Ok(signature)
    }

    /// Sign and base64-encode, the wire format both gateways use.
    pub(crate) fn sign_base64(&self, message: &[u8]) -> Result<String, PayError> {
        self.sign(message).map(|signature| b64_encode(&signature))
    }
}

/// RSA-SHA256 (PKCS#1 v1.5) verifier over a platform public key.
#[derive(Clone)]
pub(crate) struct RsaVerifier {
    /// PKCS#1 `RSAPublicKey` DER, the layout `ring` verifies against.
    pkcs1: Vec<u8>,
}

impl std::fmt::Debug for RsaVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RsaVerifier(..)")
    }
}

impl RsaVerifier {
    /// From an X.509 `SubjectPublicKeyInfo` DER document.
    pub(crate) fn from_spki_der(der: &[u8]) -> Result<Self, PayError> {
        let (_, spki) = SubjectPublicKeyInfo::from_der(der)
            .map_err(|error| PayError::Config(format!("invalid SubjectPublicKeyInfo: {error}")))?;
        Ok(Self {
            pkcs1: spki.subject_public_key.data.to_vec(),
        })
    }

    /// From a `PUBLIC KEY` PEM document or the bare base64 SPKI body Alipay
    /// consoles hand out.
    pub(crate) fn from_public_key_pem(pem: &str) -> Result<Self, PayError> {
        let mut reader = pem.as_bytes();
        for item in rustls_pemfile::read_all(&mut reader).flatten() {
            if let rustls_pemfile::Item::SubjectPublicKeyInfo(der) = &item {
                return Self::from_spki_der(der.as_ref());
            }
        }
        let compact: String = pem.split_whitespace().collect();
        let der = b64_decode(&compact)
            .map_err(|_| PayError::Config("public key is neither PEM nor base64 DER".to_owned()))?;
        Self::from_spki_der(&der)
    }

    /// From an X.509 certificate DER; also returns the certificate serial
    /// number as uppercase hex (`WeChat`'s `serial_no` format).
    pub(crate) fn from_x509_der(der: &[u8]) -> Result<(Self, String), PayError> {
        let (_, certificate) = x509_parser::parse_x509_certificate(der)
            .map_err(|error| PayError::Config(format!("invalid X.509 certificate: {error}")))?;
        let verifier = Self {
            pkcs1: certificate
                .tbs_certificate
                .subject_pki
                .subject_public_key
                .data
                .to_vec(),
        };
        let serial = hex_upper(certificate.tbs_certificate.raw_serial());
        Ok((verifier, serial))
    }

    /// From a `CERTIFICATE` PEM document (platform certificate files).
    pub(crate) fn from_x509_pem(pem: &str) -> Result<(Self, String), PayError> {
        let mut reader = pem.as_bytes();
        for item in rustls_pemfile::read_all(&mut reader).flatten() {
            if let rustls_pemfile::Item::X509Certificate(der) = &item {
                return Self::from_x509_der(der.as_ref());
            }
        }
        Err(PayError::Config(
            "no CERTIFICATE block found in platform certificate PEM".to_owned(),
        ))
    }

    /// Whether `signature` is a valid RSA-SHA256 signature of `message`.
    pub(crate) fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, &self.pkcs1)
            .verify(message, signature)
            .is_ok()
    }
}

/// AES-256-GCM decryption (`WeChat` `APIv3` resource / certificate cipher).
///
/// Errors are plain strings so callers can wrap them in the right
/// [`PayError`] variant for their context.
pub(crate) fn aes256_gcm_decrypt(
    key: &[u8],
    nonce: &[u8],
    associated_data: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| format!("APIv3 key must be 32 bytes, got {}", key.len()))?;
    if nonce.len() != 12 {
        return Err(format!(
            "AES-GCM nonce must be 12 bytes, got {}",
            nonce.len()
        ));
    }
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: associated_data,
            },
        )
        .map_err(|_| {
            "AES-256-GCM authentication failed (wrong key, nonce, or tampered data)".to_owned()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERCHANT_KEY: &str = include_str!("../tests/fixtures/wechat_merchant_key.pem");
    const MERCHANT_PUB: &str = include_str!("../tests/fixtures/wechat_merchant_pub.pem");
    const PLATFORM_KEY: &str = include_str!("../tests/fixtures/wechat_platform_key.pem");
    const PLATFORM_CERT: &str = include_str!("../tests/fixtures/wechat_platform_cert.pem");
    const ALIPAY_APP_KEY: &str = include_str!("../tests/fixtures/alipay_app_key.pem");
    const ALIPAY_APP_PUB: &str = include_str!("../tests/fixtures/alipay_app_pub.pem");

    #[test]
    fn pkcs8_sign_verify_loopback_and_tamper_detection() {
        let signer = RsaSigner::from_pem(MERCHANT_KEY).expect("pkcs8 key");
        let verifier = RsaVerifier::from_public_key_pem(MERCHANT_PUB).expect("public key");
        let message = b"GET\n/v3/certificates\n1700000000\nNONCE\n\n";
        let signature = signer.sign(message).expect("sign");
        assert!(verifier.verify(message, &signature));
        assert!(!verifier.verify(b"tampered message", &signature));
        let mut broken = signature.clone();
        broken[0] ^= 0x01;
        assert!(!verifier.verify(message, &broken));
        assert!(!verifier.verify(message, &signature[1..]));
    }

    #[test]
    fn pkcs1_key_and_bare_base64_keys_load() {
        // PKCS#1 PEM (openssl genrsa -traditional).
        let signer = RsaSigner::from_pem(ALIPAY_APP_KEY).expect("pkcs1 key");
        let verifier = RsaVerifier::from_public_key_pem(ALIPAY_APP_PUB).expect("public key");
        let signature = signer.sign(b"alipay").expect("sign");
        assert!(verifier.verify(b"alipay", &signature));

        // Bare base64 bodies (Alipay-console style, no PEM armor).
        let bare_key: String = ALIPAY_APP_KEY
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let bare_pub: String = ALIPAY_APP_PUB
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let signer = RsaSigner::from_pem(&bare_key).expect("bare base64 key");
        let verifier = RsaVerifier::from_public_key_pem(&bare_pub).expect("bare base64 pub");
        let signature = signer.sign(b"bare").expect("sign");
        assert!(verifier.verify(b"bare", &signature));

        assert!(RsaSigner::from_pem("not a key").is_err());
        assert!(RsaVerifier::from_public_key_pem("not a key").is_err());
    }

    #[test]
    fn x509_certificate_provides_serial_and_verifier() {
        let signer = RsaSigner::from_pem(PLATFORM_KEY).expect("platform key");
        let (verifier, serial) = RsaVerifier::from_x509_pem(PLATFORM_CERT).expect("cert");
        assert_eq!(serial, "5157F09EFDC096DE15EBE81A47057A7232156733");
        let message = b"1700000000\nNONCE\n{\"code\":\"SUCCESS\"}\n";
        let signature = signer.sign(message).expect("sign");
        assert!(verifier.verify(message, &signature));
        assert!(!verifier.verify(b"other", &signature));
        assert!(RsaVerifier::from_x509_pem(MERCHANT_PUB).is_err());
    }

    #[test]
    fn aes256_gcm_round_trip_and_tamper_detection() {
        let key = b"0123456789abcdef0123456789abcdef";
        let nonce = b"unique-nonce";
        let aad = b"transaction";
        let plaintext = br#"{"out_trade_no":"T1","trade_state":"SUCCESS"}"#;
        let cipher = Aes256Gcm::new_from_slice(key).expect("cipher");
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("encrypt");

        let decrypted = aes256_gcm_decrypt(key, nonce, aad, &ciphertext).expect("decrypt");
        assert_eq!(decrypted, plaintext);

        let mut tampered = ciphertext.clone();
        tampered[0] ^= 0x01;
        assert!(aes256_gcm_decrypt(key, nonce, aad, &tampered).is_err());
        assert!(aes256_gcm_decrypt(key, nonce, b"other-aad", &ciphertext).is_err());
        assert!(aes256_gcm_decrypt(b"short-key", nonce, aad, &ciphertext).is_err());
        assert!(aes256_gcm_decrypt(key, b"bad", aad, &ciphertext).is_err());
    }

    #[test]
    fn gmt8_datetime_matches_known_values() {
        assert_eq!(gmt8_datetime(0), "1970-01-01 08:00:00");
        assert_eq!(gmt8_datetime(1_700_000_000), "2023-11-15 06:13:20");
        assert_eq!(gmt8_datetime(946_684_800), "2000-01-01 08:00:00");
        // 2000-02-28 16:00:00 UTC rolls into the leap day in GMT+8.
        assert_eq!(gmt8_datetime(951_753_600), "2000-02-29 00:00:00");
    }

    #[test]
    fn helpers_encode_as_expected() {
        assert_eq!(hex_upper(&[0x51, 0x57, 0xf0, 0x9e]), "5157F09E");
        assert_eq!(b64_encode(b"phoenix"), "cGhvZW5peA==");
        assert_eq!(
            b64_decode("cGhvZW5peA==").as_deref(),
            Ok(b"phoenix".as_slice())
        );
        assert!(b64_decode("!!!").is_err());
        let nonce_a = random_nonce().expect("nonce");
        let nonce_b = random_nonce().expect("nonce");
        assert_eq!(nonce_a.len(), 32);
        assert_ne!(nonce_a, nonce_b);
    }
}
