use std::fmt::Debug;

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use serde::Serialize;
use serde_json::Value;

const PAGE_MEDIA_TYPE: &str = "application/vnd.phoenix.page+json";

/// Buffered HTTP response with fluent test assertions.
#[derive(Clone, Debug)]
pub struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl TestResponse {
    pub(crate) fn new(status: StatusCode, headers: HeaderMap, body: Bytes) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// Response status code.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    /// Response headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Raw response body.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Body decoded as UTF-8 lossy text.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Parse the body as JSON.
    ///
    /// # Panics
    ///
    /// Panics when the body is not valid JSON.
    #[must_use]
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("response body should be JSON")
    }

    /// Assert the status is in the 2xx range.
    ///
    /// # Panics
    ///
    /// Panics when the status is outside 200–299.
    pub fn assert_ok(&self) -> &Self {
        assert!(
            self.status.is_success(),
            "expected 2xx status, got {} body={}",
            self.status,
            self.text()
        );
        self
    }

    /// Assert an exact status code.
    ///
    /// # Panics
    ///
    /// Panics when the status differs from `expected`.
    pub fn assert_status(&self, expected: StatusCode) -> &Self {
        assert_eq!(
            self.status,
            expected,
            "unexpected status; body={}",
            self.text()
        );
        self
    }

    /// Assert the body contains `needle` as UTF-8 text.
    ///
    /// # Panics
    ///
    /// Panics when `needle` is absent from the body.
    pub fn assert_body_contains(&self, needle: &str) -> &Self {
        let text = self.text();
        assert!(
            text.contains(needle),
            "body does not contain `{needle}`: {text}"
        );
        self
    }

    /// Run a custom assertion against the JSON body.
    ///
    /// # Panics
    ///
    /// Panics when the body is not JSON, or when `check` panics.
    pub fn assert_json(&self, check: impl FnOnce(&Value)) -> &Self {
        let value = self.json();
        check(&value);
        self
    }

    /// Assert that a dotted JSON path equals `expected`.
    ///
    /// # Panics
    ///
    /// Panics when the path is missing or the value differs.
    pub fn assert_json_path<T>(&self, path: &str, expected: T) -> &Self
    where
        T: Serialize + Debug,
    {
        let value = self.json();
        let actual = json_path(&value, path).unwrap_or_else(|| {
            panic!("JSON path `{path}` not found in {value}");
        });
        let expected = serde_json::to_value(expected).expect("expected value should serialize");
        assert_eq!(
            actual, &expected,
            "JSON path `{path}` mismatch: left=actual right=expected"
        );
        self
    }

    /// Assert a page-protocol response names `page`.
    ///
    /// Accepts either `application/vnd.phoenix.page+json` with a `page` field,
    /// or an HTML envelope that embeds `"page":"..."`.
    ///
    /// # Panics
    ///
    /// Panics when the response is not a page payload for `page`.
    pub fn assert_page(&self, page: &str) -> &Self {
        let content_type = self
            .headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();

        if content_type.starts_with(PAGE_MEDIA_TYPE) {
            let value = self.json();
            let actual = value
                .get("page")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("page protocol JSON missing string `page`: {value}"));
            assert_eq!(actual, page, "unexpected page name");
            return self;
        }

        let text = self.text();
        let needle = format!("\"page\":\"{page}\"");
        assert!(
            text.contains(&needle) || text.contains(&format!("\"page\": \"{page}\"")),
            "response is not a page protocol payload for `{page}`: content-type={content_type} body={text}"
        );
        self
    }
}

fn json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        current = current.get(segment)?;
    }
    Some(current)
}
