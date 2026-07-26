//! Alipay `OpenAPI` (RSA2) protocol pieces: canonical sign-content assembly,
//! synchronous-response content extraction, and `trade_status` mapping.
//! Pure functions, unit-tested offline.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{PayError, PaymentStatus};

/// Build the request sign content: all parameters except `sign`, empty values
/// skipped, sorted by key (`BTreeMap` order), joined `k=v&k=v`, values raw
/// (not URL-encoded). `sign_type` IS part of request signing.
pub(crate) fn request_sign_content(params: &BTreeMap<String, String>) -> String {
    join_params(params, &["sign"])
}

/// Build the asynchronous-notify verification content: all parameters except
/// `sign` and `sign_type`, sorted by key, joined `k=v&k=v` with the decoded
/// values.
pub(crate) fn notify_sign_content(params: &BTreeMap<String, String>) -> String {
    join_params(params, &["sign", "sign_type"])
}

fn join_params(params: &BTreeMap<String, String>, skip: &[&str]) -> String {
    let mut content = String::new();
    for (key, value) in params {
        if skip.contains(&key.as_str()) || value.is_empty() {
            continue;
        }
        if !content.is_empty() {
            content.push('&');
        }
        content.push_str(key);
        content.push('=');
        content.push_str(value);
    }
    content
}

/// Extract the raw `"<key>": { ... }` object substring of a synchronous
/// response — the exact bytes the platform signed. Brace matching is
/// string-aware so values containing `{`/`}` cannot desynchronize it.
pub(crate) fn extract_response_content<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\"");
    let key_start = body.find(&marker)?;
    let after_key = key_start + marker.len();
    let brace_offset = body[after_key..].find('{')?;
    let content_start = after_key + brace_offset;

    let bytes = body.as_bytes();
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate().skip(content_start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&body[content_start..=index]);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Map Alipay `trade_status` to the Phoenix state machine.
pub(crate) fn map_trade_status(trade_status: &str) -> Result<PaymentStatus, PayError> {
    match trade_status {
        "TRADE_SUCCESS" | "TRADE_FINISHED" => Ok(PaymentStatus::Paid),
        "WAIT_BUYER_PAY" => Ok(PaymentStatus::Pending),
        "TRADE_CLOSED" => Ok(PaymentStatus::Closed),
        other => Err(PayError::InvalidNotify(format!(
            "unknown Alipay trade_status `{other}`"
        ))),
    }
}

/// Parsed `alipay_trade_*_response` payload (superset across precreate /
/// query / close).
#[derive(Debug, Deserialize)]
pub(crate) struct ResponsePayload {
    pub code: String,
    #[serde(default)]
    pub msg: String,
    #[serde(default)]
    pub sub_code: String,
    #[serde(default)]
    pub sub_msg: String,
    #[serde(default)]
    pub qr_code: Option<String>,
    #[serde(default)]
    pub trade_status: Option<String>,
    /// `alipay.trade.refund`: amount actually refunded this call, in yuan.
    #[serde(default)]
    pub refund_fee: Option<String>,
    /// `alipay.trade.fastpay.refund.query`: total refunded so far, in yuan.
    #[serde(default)]
    pub refund_amount: Option<String>,
    /// `alipay.trade.fastpay.refund.query`: `REFUND_SUCCESS` when settled.
    #[serde(default)]
    pub refund_status: Option<String>,
    /// Provider-side trade number, echoed by refund calls.
    #[serde(default)]
    pub trade_no: Option<String>,
    /// Merchant refund number, echoed by refund calls.
    #[serde(default)]
    pub out_request_no: Option<String>,
    /// Bill download URL (`alipay.data.dataservice.bill.downloadurl.query`).
    #[serde(default)]
    pub bill_download_url: Option<String>,
}

impl ResponsePayload {
    /// Gateway-level success (`code == "10000"`).
    pub(crate) fn is_success(&self) -> bool {
        self.code == "10000"
    }

    /// One-line business error description for [`PayError::Gateway`].
    pub(crate) fn error_text(&self, method: &str) -> String {
        format!(
            "{method} failed: code={} msg={} sub_code={} sub_msg={}",
            self.code, self.msg, self.sub_code, self.sub_msg
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn request_sign_content_sorts_skips_empty_and_keeps_sign_type() {
        let content = request_sign_content(&params(&[
            ("method", "alipay.trade.precreate"),
            ("app_id", "2021"),
            ("sign", "SHOULD_BE_SKIPPED"),
            ("sign_type", "RSA2"),
            ("empty", ""),
            (
                "biz_content",
                r#"{"out_trade_no":"T1","total_amount":"1.00"}"#,
            ),
        ]));
        assert_eq!(
            content,
            "app_id=2021&biz_content={\"out_trade_no\":\"T1\",\"total_amount\":\"1.00\"}\
             &method=alipay.trade.precreate&sign_type=RSA2"
        );
    }

    #[test]
    fn notify_sign_content_drops_sign_and_sign_type() {
        let content = notify_sign_content(&params(&[
            ("trade_status", "TRADE_SUCCESS"),
            ("out_trade_no", "T1"),
            ("sign", "x"),
            ("sign_type", "RSA2"),
        ]));
        assert_eq!(content, "out_trade_no=T1&trade_status=TRADE_SUCCESS");
    }

    #[test]
    fn response_content_extraction_is_exact_and_string_aware() {
        let body = concat!(
            r#"{"alipay_trade_precreate_response":"#,
            r#"{"code":"10000","msg":"Success{\"tricky\":1}","out_trade_no":"T{1}","#,
            r#""nested":{"a":"}"},"qr_code":"https://qr.alipay.com/x"},"#,
            r#""sign":"BASE64=="}"#
        );
        let content =
            extract_response_content(body, "alipay_trade_precreate_response").expect("content");
        assert!(content.starts_with(r#"{"code":"10000""#));
        assert!(content.ends_with(r#""qr_code":"https://qr.alipay.com/x"}"#));
        let payload: ResponsePayload = serde_json::from_str(content).expect("payload");
        assert!(payload.is_success());
        assert_eq!(payload.qr_code.as_deref(), Some("https://qr.alipay.com/x"));

        assert!(extract_response_content(body, "alipay_trade_query_response").is_none());
        assert!(extract_response_content("{\"x\": 1}", "x").is_none());
        assert!(extract_response_content("{\"k\":{", "k").is_none());
    }

    #[test]
    fn trade_statuses_map_to_the_state_machine() {
        assert_eq!(map_trade_status("TRADE_SUCCESS"), Ok(PaymentStatus::Paid));
        assert_eq!(map_trade_status("TRADE_FINISHED"), Ok(PaymentStatus::Paid));
        assert_eq!(
            map_trade_status("WAIT_BUYER_PAY"),
            Ok(PaymentStatus::Pending)
        );
        assert_eq!(map_trade_status("TRADE_CLOSED"), Ok(PaymentStatus::Closed));
        assert!(map_trade_status("NOPE").is_err());
    }
}
