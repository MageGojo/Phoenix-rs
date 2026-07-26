//! AWS Signature Version 4 (`AWS4-HMAC-SHA256`) signing, implemented from the
//! specification with `hmac` + `sha2` (no `aws-sdk` / `rusoto`).
//!
//! Two signing modes are provided, both used by [`crate::S3Disk`]:
//!
//! - [`SigV4::sign_headers`] — `Authorization`-header signing for live
//!   PUT/GET/DELETE/HEAD requests (payload hashed into
//!   `x-amz-content-sha256`).
//! - [`SigV4::presign`] — query-string signing for presigned URLs
//!   (`X-Amz-Signature=…`, `UNSIGNED-PAYLOAD`), so a browser or CDN can talk
//!   to S3 directly without the secret key.
//!
//! The signing pipeline is the canonical four steps: canonical request →
//! string to sign → derived signing key → HMAC-SHA256 signature. Everything
//! here is pure (no clock, no network); the caller supplies the timestamp so
//! unit tests can pin the AWS-documented test vectors exactly (see the tests
//! at the bottom of this file).

use hmac::{Hmac, KeyInit, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};

/// The signing algorithm identifier used in the credential scope and headers.
pub(crate) const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// Sentinel payload hash used for presigned URLs (body is not signed).
pub(crate) const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

type HmacSha256 = Hmac<Sha256>;

/// RFC 3986 unreserved characters are `A-Za-z0-9-._~`; `SigV4` percent-encodes
/// everything else. This set marks the bytes that MUST be encoded.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Same as [`UNRESERVED`] but leaves `/` untouched, for encoding URI paths
/// (S3 encodes the path once, keeping the segment separators).
const UNRESERVED_PATH: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// Percent-encode `value` per RFC 3986 (`SigV4` rules). When `keep_slash` is
/// true, `/` is preserved (used for canonical URI paths); otherwise `/` is
/// encoded as `%2F` (used for query values such as the credential scope).
pub(crate) fn uri_encode(value: &str, keep_slash: bool) -> String {
    let set = if keep_slash {
        UNRESERVED_PATH
    } else {
        UNRESERVED
    };
    utf8_percent_encode(value, set).to_string()
}

/// Lowercase hex encoding.
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Lowercase hex SHA-256 of `data`.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    to_hex(&Sha256::digest(data))
}

/// HMAC-SHA256 of `data` under `key`.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    let mut out = [0_u8; 32];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

/// Format a unix timestamp as `SigV4` needs it: the `x-amz-date` value
/// (`YYYYMMDDTHHMMSSZ`) and the credential-scope date stamp (`YYYYMMDD`),
/// both in UTC. Integer-only civil-from-days (Howard Hinnant's algorithm).
pub(crate) fn format_timestamp(unix_seconds: u64) -> (String, String) {
    let days = unix_seconds / 86_400;
    let secs = unix_seconds % 86_400;
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    let (hour, minute, second) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    (
        format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"),
        format!("{year:04}{month:02}{day:02}"),
    )
}

/// Current unix timestamp in seconds.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The immutable inputs shared by every signature: credentials plus the
/// region / service / date that make up the credential scope.
pub(crate) struct SigV4<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    pub amz_date: &'a str,
    pub date_stamp: &'a str,
}

/// Result of an `Authorization`-header signature.
///
/// The driver only consumes `authorization`; the remaining fields are the
/// standard `SigV4` intermediates, kept for diagnostics and asserted by the
/// unit tests against the AWS documented vectors.
#[allow(dead_code)]
pub(crate) struct HeaderSignature {
    pub authorization: String,
    pub signed_headers: String,
    pub signature: String,
    pub canonical_request: String,
    pub string_to_sign: String,
}

/// Result of a query-string (presigned URL) signature.
///
/// The driver only consumes `query`; the remaining fields are `SigV4`
/// intermediates asserted by the unit tests.
#[allow(dead_code)]
pub(crate) struct PresignedQuery {
    /// The full canonical query string, including the trailing
    /// `&X-Amz-Signature=…`.
    pub query: String,
    pub signature: String,
    pub canonical_request: String,
    pub string_to_sign: String,
}

impl SigV4<'_> {
    /// `YYYYMMDD/region/service/aws4_request`.
    fn scope(&self) -> String {
        format!(
            "{}/{}/{}/aws4_request",
            self.date_stamp, self.region, self.service
        )
    }

    /// Derive the `SigV4` signing key: HMAC chained over `AWS4`+secret, date,
    /// region, service, and the literal `aws4_request`.
    fn signing_key(&self) -> [u8; 32] {
        let k_date = hmac_sha256(
            format!("AWS4{}", self.secret_key).as_bytes(),
            self.date_stamp.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, self.service.as_bytes());
        hmac_sha256(&k_service, b"aws4_request")
    }

    /// Build the string to sign from an already-hashed canonical request.
    fn string_to_sign(&self, canonical_request: &str) -> String {
        format!(
            "{ALGORITHM}\n{}\n{}\n{}",
            self.amz_date,
            self.scope(),
            sha256_hex(canonical_request.as_bytes())
        )
    }

    /// `signature = hex(HMAC(signing_key, string_to_sign))`.
    fn sign(&self, string_to_sign: &str) -> String {
        to_hex(&hmac_sha256(&self.signing_key(), string_to_sign.as_bytes()))
    }

    /// Sign a request with an `Authorization` header.
    ///
    /// `headers` are `(lowercase-name, value)` pairs that will be sorted and
    /// signed (they must include `host`). `canonical_uri` is the already
    /// percent-encoded path and `canonical_query` the already-canonical query
    /// string (empty for keyless requests). `payload_hash` is the lowercase
    /// hex SHA-256 of the body.
    pub(crate) fn sign_headers(
        &self,
        method: &str,
        canonical_uri: &str,
        canonical_query: &str,
        headers: &[(String, String)],
        payload_hash: &str,
    ) -> HeaderSignature {
        let (canonical_headers, signed_headers) = canonical_headers(headers);
        let canonical_request = format!(
            "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );
        let string_to_sign = self.string_to_sign(&canonical_request);
        let signature = self.sign(&string_to_sign);
        let authorization = format!(
            "{ALGORITHM} Credential={}/{}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key,
            self.scope(),
        );
        HeaderSignature {
            authorization,
            signed_headers,
            signature,
            canonical_request,
            string_to_sign,
        }
    }

    /// Produce a presigned query string for `method` on `canonical_uri`,
    /// valid for `expires_secs`, signing only the `host` header.
    pub(crate) fn presign(
        &self,
        method: &str,
        canonical_uri: &str,
        host: &str,
        expires_secs: u64,
    ) -> PresignedQuery {
        let credential = format!("{}/{}", self.access_key, self.scope());
        // Query parameters are sorted by (encoded) key; our keys are ASCII so
        // they encode to themselves, but we still encode for correctness.
        let params: [(&str, String); 5] = [
            ("X-Amz-Algorithm", ALGORITHM.to_owned()),
            ("X-Amz-Credential", credential),
            ("X-Amz-Date", self.amz_date.to_owned()),
            ("X-Amz-Expires", expires_secs.to_string()),
            ("X-Amz-SignedHeaders", "host".to_owned()),
        ];
        let mut encoded: Vec<(String, String)> = params
            .iter()
            .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
            .collect();
        encoded.sort();
        let canonical_query = encoded
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let canonical_request = format!(
            "{method}\n{canonical_uri}\n{canonical_query}\nhost:{host}\n\nhost\n{UNSIGNED_PAYLOAD}"
        );
        let string_to_sign = self.string_to_sign(&canonical_request);
        let signature = self.sign(&string_to_sign);
        PresignedQuery {
            query: format!("{canonical_query}&X-Amz-Signature={signature}"),
            signature,
            canonical_request,
            string_to_sign,
        }
    }
}

/// Build the canonical headers block and the signed-headers list from
/// `(name, value)` pairs. Names are lowercased, values trimmed, and both the
/// block and the list are sorted by header name.
fn canonical_headers(headers: &[(String, String)]) -> (String, String) {
    let mut sorted: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let signed_headers = sorted
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let mut block = String::new();
    for (name, value) in &sorted {
        block.push_str(name);
        block.push(':');
        block.push_str(value);
        block.push('\n');
    }
    (block, signed_headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Credentials from the AWS S3 SigV4 documentation examples.
    const ACCESS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
    const SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    const REGION: &str = "us-east-1";
    const SERVICE: &str = "s3";
    const AMZ_DATE: &str = "20130524T000000Z";
    const DATE_STAMP: &str = "20130524";

    fn signer() -> SigV4<'static> {
        SigV4 {
            access_key: ACCESS_KEY,
            secret_key: SECRET_KEY,
            region: REGION,
            service: SERVICE,
            amz_date: AMZ_DATE,
            date_stamp: DATE_STAMP,
        }
    }

    #[test]
    fn signing_key_derivation_is_deterministic_and_scope_sensitive() {
        // The derived signing key is validated *transitively* by the three
        // AWS S3 vectors below (they reproduce the exact documented final
        // signatures, which cannot happen unless `signing_key()` is correct).
        // Here we additionally pin the structural properties: derivation is
        // deterministic, and any change to date / region / service produces a
        // different key (so the credential scope really binds the signature).
        let base = signer();
        assert_eq!(base.signing_key(), signer().signing_key());

        let other_region = SigV4 {
            region: "us-west-2",
            ..signer()
        };
        let other_service = SigV4 {
            service: "iam",
            ..signer()
        };
        let other_date = SigV4 {
            date_stamp: "20130525",
            ..signer()
        };
        assert_ne!(base.signing_key(), other_region.signing_key());
        assert_ne!(base.signing_key(), other_service.signing_key());
        assert_ne!(base.signing_key(), other_date.signing_key());
    }

    #[test]
    fn empty_and_known_sha256_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn format_timestamp_matches_vector_epoch() {
        // 2013-05-24T00:00:00Z == 1_369_353_600.
        assert_eq!(
            format_timestamp(1_369_353_600),
            ("20130524T000000Z".to_owned(), "20130524".to_owned())
        );
        assert_eq!(
            format_timestamp(0),
            ("19700101T000000Z".to_owned(), "19700101".to_owned())
        );
    }

    #[test]
    fn uri_encode_rules() {
        assert_eq!(uri_encode("test$file.text", true), "test%24file.text");
        assert_eq!(uri_encode("/a/b c", true), "/a/b%20c");
        // Query context encodes the slash.
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(uri_encode("-._~", false), "-._~");
    }

    /// AWS docs — "Example: GET Object". Verifies canonical request, string to
    /// sign, and the final signature byte-for-byte.
    #[test]
    fn aws_vector_get_object() {
        let headers = vec![
            (
                "host".to_owned(),
                "examplebucket.s3.amazonaws.com".to_owned(),
            ),
            ("range".to_owned(), "bytes=0-9".to_owned()),
            ("x-amz-content-sha256".to_owned(), sha256_hex(b"")),
            ("x-amz-date".to_owned(), AMZ_DATE.to_owned()),
        ];
        let signed = signer().sign_headers("GET", "/test.txt", "", &headers, &sha256_hex(b""));

        assert_eq!(
            signed.canonical_request,
            "GET\n\
             /test.txt\n\
             \n\
             host:examplebucket.s3.amazonaws.com\n\
             range:bytes=0-9\n\
             x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
             x-amz-date:20130524T000000Z\n\
             \n\
             host;range;x-amz-content-sha256;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            signed.string_to_sign,
            "AWS4-HMAC-SHA256\n\
             20130524T000000Z\n\
             20130524/us-east-1/s3/aws4_request\n\
             7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );
        assert_eq!(
            signed.signature,
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
        assert!(
            signed
                .authorization
                .contains("Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request")
        );
        assert!(
            signed
                .authorization
                .contains("SignedHeaders=host;range;x-amz-content-sha256;x-amz-date")
        );
    }

    /// AWS docs — "Example: PUT Object". The `$` in the key must be encoded to
    /// `%24` in the canonical URI.
    #[test]
    fn aws_vector_put_object() {
        let body = b"Welcome to Amazon S3.";
        let payload_hash = sha256_hex(body);
        assert_eq!(
            payload_hash,
            "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072"
        );
        let headers = vec![
            (
                "date".to_owned(),
                "Fri, 24 May 2013 00:00:00 GMT".to_owned(),
            ),
            (
                "host".to_owned(),
                "examplebucket.s3.amazonaws.com".to_owned(),
            ),
            ("x-amz-content-sha256".to_owned(), payload_hash.clone()),
            ("x-amz-date".to_owned(), AMZ_DATE.to_owned()),
            (
                "x-amz-storage-class".to_owned(),
                "REDUCED_REDUNDANCY".to_owned(),
            ),
        ];
        let signed = signer().sign_headers("PUT", "/test%24file.text", "", &headers, &payload_hash);

        assert_eq!(
            signed.string_to_sign,
            "AWS4-HMAC-SHA256\n\
             20130524T000000Z\n\
             20130524/us-east-1/s3/aws4_request\n\
             9e0e90d9c76de8fa5b200d8c849cd5b8dc7a3be3951ddb7f6a76b4158342019d"
        );
        assert_eq!(
            signed.signature,
            "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        );
    }

    /// AWS docs — presigned GET URL example (`X-Amz-Expires=86400`).
    #[test]
    fn aws_vector_presigned_get() {
        let presigned =
            signer().presign("GET", "/test.txt", "examplebucket.s3.amazonaws.com", 86_400);

        assert_eq!(
            presigned.canonical_request,
            "GET\n\
             /test.txt\n\
             X-Amz-Algorithm=AWS4-HMAC-SHA256&\
             X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&\
             X-Amz-Date=20130524T000000Z&\
             X-Amz-Expires=86400&\
             X-Amz-SignedHeaders=host\n\
             host:examplebucket.s3.amazonaws.com\n\
             \n\
             host\n\
             UNSIGNED-PAYLOAD"
        );
        assert_eq!(
            presigned.signature,
            "aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
        assert!(
            presigned
                .query
                .ends_with(&format!("&X-Amz-Signature={}", presigned.signature))
        );
    }
}
