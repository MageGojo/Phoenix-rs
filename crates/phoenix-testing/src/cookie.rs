use std::collections::HashMap;

use http::{HeaderMap, HeaderValue, header};

/// Simple name→value cookie jar for test request/response round trips.
#[derive(Clone, Debug, Default)]
pub(crate) struct CookieJar {
    cookies: HashMap<String, String>,
}

impl CookieJar {
    pub(crate) fn store_from_response(&mut self, headers: &HeaderMap) {
        for value in headers.get_all(header::SET_COOKIE) {
            let Ok(raw) = value.to_str() else {
                continue;
            };
            let Some(pair) = raw.split(';').next() else {
                continue;
            };
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            self.cookies
                .insert(name.to_owned(), value.trim().to_owned());
        }
    }

    pub(crate) fn header_value(&self) -> Option<HeaderValue> {
        if self.cookies.is_empty() {
            return None;
        }
        let mut parts = self
            .cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>();
        parts.sort();
        HeaderValue::from_str(&parts.join("; ")).ok()
    }
}
