//! Server-Sent Events response types.

use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use http::{HeaderName, HeaderValue, header};
use serde::Serialize;
use thiserror::Error;

use crate::{
    FromRequest, IntoResponse, Request, Response, ResponseBodyError, StatusCode, rejection_response,
};

const DEFAULT_SSE_EVENT_LIMIT: usize = 64 * 1024;
const MAX_SSE_EVENT_LIMIT: usize = 1024 * 1024;
const MAX_SSE_FIELD_LENGTH: usize = 1024;
const MAX_KEEP_ALIVE_INTERVAL: Duration = Duration::from_hours(24);

/// One validated Server-Sent Event.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SseEvent {
    event: Option<String>,
    id: Option<String>,
    data: String,
    retry: Option<Duration>,
    comment: Option<String>,
}

impl SseEvent {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the browser event type.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, NUL, CR, or LF values.
    pub fn event(mut self, event: impl Into<String>) -> Result<Self, InvalidSseField> {
        self.event = Some(validate_sse_field("event", event.into(), false)?);
        Ok(self)
    }

    /// Set the reconnection cursor sent back as `Last-Event-ID`.
    ///
    /// # Errors
    ///
    /// Rejects oversized, NUL, CR, or LF values.
    pub fn id(mut self, id: impl Into<String>) -> Result<Self, InvalidSseField> {
        self.id = Some(validate_sse_field("id", id.into(), true)?);
        Ok(self)
    }

    #[must_use]
    pub fn data(mut self, data: impl Into<String>) -> Self {
        self.data = data.into();
        self
    }

    /// Serialize structured data into one SSE data field.
    ///
    /// # Errors
    ///
    /// Returns the serializer error for unsupported values.
    pub fn json_data<T>(mut self, data: &T) -> Result<Self, serde_json::Error>
    where
        T: Serialize,
    {
        self.data = serde_json::to_string(data)?;
        Ok(self)
    }

    #[must_use]
    pub const fn retry(mut self, retry: Duration) -> Self {
        self.retry = Some(retry);
        self
    }

    /// Add a single-line SSE comment before the event fields.
    ///
    /// # Errors
    ///
    /// Rejects oversized, NUL, CR, or LF values.
    pub fn comment(mut self, comment: impl Into<String>) -> Result<Self, InvalidSseField> {
        self.comment = Some(validate_sse_field("comment", comment.into(), true)?);
        Ok(self)
    }

    fn encode(self, max_size: usize) -> Result<Bytes, ResponseBodyError> {
        let mut output = String::new();
        if let Some(comment) = self.comment {
            output.push_str(": ");
            output.push_str(&comment);
            output.push('\n');
        }
        if let Some(event) = self.event {
            output.push_str("event: ");
            output.push_str(&event);
            output.push('\n');
        }
        if let Some(id) = self.id {
            output.push_str("id: ");
            output.push_str(&id);
            output.push('\n');
        }
        if let Some(retry) = self.retry {
            output.push_str("retry: ");
            output.push_str(
                &u64::try_from(retry.as_millis())
                    .unwrap_or(u64::MAX)
                    .to_string(),
            );
            output.push('\n');
        }
        let data = self.data.replace("\r\n", "\n").replace('\r', "\n");
        for line in data.split('\n') {
            output.push_str("data: ");
            output.push_str(line);
            output.push('\n');
        }
        output.push('\n');
        if output.len() > max_size {
            return Err(ResponseBodyError);
        }
        Ok(Bytes::from(output))
    }
}

/// Validated SSE keepalive policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeepAlive {
    interval: Duration,
    comment: String,
}

impl KeepAlive {
    /// Configure a keepalive interval.
    ///
    /// # Errors
    ///
    /// Rejects zero or intervals above 24 hours.
    pub fn new(interval: Duration) -> Result<Self, SseConfigError> {
        if interval.is_zero() || interval > MAX_KEEP_ALIVE_INTERVAL {
            return Err(SseConfigError::InvalidKeepAliveInterval);
        }
        Ok(Self {
            interval,
            comment: "keep-alive".to_owned(),
        })
    }

    /// Set the single-line keepalive comment.
    ///
    /// # Errors
    ///
    /// Rejects oversized, NUL, CR, or LF values.
    pub fn comment(mut self, comment: impl Into<String>) -> Result<Self, InvalidSseField> {
        self.comment = validate_sse_field("comment", comment.into(), true)?;
        Ok(self)
    }
}

impl Default for KeepAlive {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(15),
            comment: "keep-alive".to_owned(),
        }
    }
}

type SseEventStream =
    Pin<Box<dyn Stream<Item = Result<SseEvent, ResponseBodyError>> + Send + 'static>>;

/// A backpressure-aware Server-Sent Events response.
pub struct Sse {
    events: SseEventStream,
    keep_alive: Option<KeepAlive>,
    max_event_size: usize,
}

impl Sse {
    /// Build SSE from a fallible event source. Source errors are redacted at the wire boundary.
    #[must_use]
    pub fn new<S, E>(events: S) -> Self
    where
        S: Stream<Item = Result<SseEvent, E>> + Send + 'static,
        E: Send + 'static,
    {
        Self {
            events: Box::pin(events.map(|event| event.map_err(|_| ResponseBodyError))),
            keep_alive: None,
            max_event_size: DEFAULT_SSE_EVENT_LIMIT,
        }
    }

    /// Build SSE from an infallible event source.
    #[must_use]
    pub fn from_events<S>(events: S) -> Self
    where
        S: Stream<Item = SseEvent> + Send + 'static,
    {
        Self::new(events.map(Ok::<_, Infallible>))
    }

    #[must_use]
    pub fn keep_alive(mut self, keep_alive: KeepAlive) -> Self {
        self.keep_alive = Some(keep_alive);
        self
    }

    /// Override the encoded per-event byte limit.
    ///
    /// # Errors
    ///
    /// Rejects zero and values above 1 MiB.
    pub fn max_event_size(mut self, bytes: usize) -> Result<Self, SseConfigError> {
        if bytes == 0 || bytes > MAX_SSE_EVENT_LIMIT {
            return Err(SseConfigError::InvalidEventSize);
        }
        self.max_event_size = bytes;
        Ok(self)
    }
}

impl std::fmt::Debug for Sse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Sse")
            .field("keep_alive", &self.keep_alive)
            .field("max_event_size", &self.max_event_size)
            .finish_non_exhaustive()
    }
}

impl IntoResponse for Sse {
    fn into_response(self) -> Response {
        let mut response = Response::try_stream(SseBodyStream::new(
            self.events,
            self.keep_alive,
            self.max_event_size,
        ));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream; charset=utf-8"),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        response.headers_mut().insert(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        );
        response
    }
}

struct SseBodyStream {
    events: SseEventStream,
    keep_alive: Option<KeepAlive>,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    max_event_size: usize,
    finished: bool,
}

impl SseBodyStream {
    fn new(events: SseEventStream, keep_alive: Option<KeepAlive>, max_event_size: usize) -> Self {
        let sleep = keep_alive
            .as_ref()
            .map(|keep_alive| Box::pin(tokio::time::sleep(keep_alive.interval)));
        Self {
            events,
            keep_alive,
            sleep,
            max_event_size,
            finished: false,
        }
    }

    fn reset_keep_alive(&mut self) {
        if let (Some(keep_alive), Some(sleep)) = (&self.keep_alive, &mut self.sleep) {
            sleep
                .as_mut()
                .reset(tokio::time::Instant::now() + keep_alive.interval);
        }
    }
}

impl Stream for SseBodyStream {
    type Item = Result<Bytes, ResponseBodyError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        if this.finished {
            return Poll::Ready(None);
        }
        match this.events.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(event))) => {
                this.reset_keep_alive();
                let encoded = event.encode(this.max_event_size);
                if encoded.is_err() {
                    this.finished = true;
                }
                return Poll::Ready(Some(encoded));
            }
            Poll::Ready(Some(Err(error))) => {
                this.finished = true;
                return Poll::Ready(Some(Err(error)));
            }
            Poll::Ready(None) => {
                this.finished = true;
                return Poll::Ready(None);
            }
            Poll::Pending => {}
        }

        if this
            .sleep
            .as_mut()
            .is_some_and(|sleep| sleep.as_mut().poll(context).is_ready())
        {
            let comment = this
                .keep_alive
                .as_ref()
                .expect("a keepalive timer has a policy")
                .comment
                .clone();
            this.reset_keep_alive();
            return Poll::Ready(Some(Ok(Bytes::from(format!(": {comment}\n\n")))));
        }
        Poll::Pending
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("The SSE {field} field is invalid.")]
pub struct InvalidSseField {
    field: &'static str,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SseConfigError {
    #[error("The SSE event size limit must be between 1 byte and 1 MiB.")]
    InvalidEventSize,
    #[error("The SSE keepalive interval must be greater than zero and at most 24 hours.")]
    InvalidKeepAliveInterval,
}

fn validate_sse_field(
    field: &'static str,
    value: String,
    allow_empty: bool,
) -> Result<String, InvalidSseField> {
    if (!allow_empty && value.is_empty())
        || value.len() > MAX_SSE_FIELD_LENGTH
        || value
            .chars()
            .any(|character| matches!(character, '\0' | '\r' | '\n'))
    {
        return Err(InvalidSseField { field });
    }
    Ok(value)
}

/// Optional browser reconnection cursor from the `Last-Event-ID` header.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LastEventId(Option<String>);

impl LastEventId {
    #[must_use]
    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }

    #[must_use]
    pub fn into_inner(self) -> Option<String> {
        self.0
    }
}

impl FromRequest for LastEventId {
    type Rejection = LastEventIdRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        let Some(value) = request.headers().get("last-event-id") else {
            return Ok(Self(None));
        };
        let value = value.to_str().map_err(|_| LastEventIdRejection)?.to_owned();
        validate_sse_field("Last-Event-ID", value, true)
            .map(|value| Self(Some(value)))
            .map_err(|_| LastEventIdRejection)
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("The Last-Event-ID header is invalid.")]
pub struct LastEventIdRejection;

impl IntoResponse for LastEventIdRejection {
    fn into_response(self) -> Response {
        rejection_response(StatusCode::BAD_REQUEST, &self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::{StreamExt, stream};
    use http::{HeaderValue, header};

    use super::*;
    use crate::{Method, Request, ResponseBody};

    #[tokio::test]
    async fn sse_encodes_validated_events_headers_limits_and_keepalives() {
        let event = SseEvent::new()
            .comment("snapshot")
            .unwrap()
            .event("member.updated")
            .unwrap()
            .id("42")
            .unwrap()
            .retry(Duration::from_millis(1_500))
            .data("first\r\nsecond\rthird\n");
        let response = Sse::from_events(stream::iter([event])).into_response();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream; charset=utf-8"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(response.headers()["x-accel-buffering"], "no");
        assert!(!response.headers().contains_key(header::CONNECTION));
        let (_, _, body) = response.into_parts();
        let ResponseBody::Stream(stream) = body else {
            panic!("expected SSE stream");
        };
        let encoded = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(Result::unwrap)
            .fold(Vec::new(), |mut output, chunk| {
                output.extend_from_slice(&chunk);
                output
            });
        assert_eq!(
            encoded,
            b": snapshot\nevent: member.updated\nid: 42\nretry: 1500\ndata: first\ndata: second\ndata: third\ndata: \n\n"
        );

        let keep_alive = KeepAlive::new(Duration::from_millis(10))
            .unwrap()
            .comment("heartbeat")
            .unwrap();
        let response = Sse::from_events(stream::pending()).keep_alive(keep_alive);
        let (_, _, body) = response.into_response().into_parts();
        let ResponseBody::Stream(mut stream) = body else {
            panic!("expected SSE stream");
        };
        let heartbeat = tokio::time::timeout(Duration::from_millis(200), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(heartbeat, ": heartbeat\n\n");

        let response = Sse::from_events(stream::iter([SseEvent::new().data("too large")]))
            .max_event_size(4)
            .unwrap()
            .into_response();
        let (_, _, body) = response.into_parts();
        let ResponseBody::Stream(mut stream) = body else {
            panic!("expected SSE stream");
        };
        assert!(stream.next().await.unwrap().is_err());
    }

    #[test]
    fn sse_fields_configuration_and_last_event_id_fail_closed() {
        for invalid in ["bad\nevent", "bad\rid", "bad\0comment"] {
            assert!(SseEvent::new().event(invalid).is_err());
            assert!(SseEvent::new().id(invalid).is_err());
            assert!(SseEvent::new().comment(invalid).is_err());
        }
        assert!(SseEvent::new().event("").is_err());
        assert!(SseEvent::new().event("x".repeat(1_025)).is_err());
        assert!(KeepAlive::new(Duration::ZERO).is_err());
        assert!(KeepAlive::new(Duration::from_secs(24 * 60 * 60 + 1)).is_err());
        assert!(Sse::from_events(stream::empty()).max_event_size(0).is_err());

        let missing = Request::new(Method::GET, "/events".parse().unwrap());
        assert_eq!(
            LastEventId::from_request(&missing).unwrap().as_deref(),
            None
        );
        let mut present = Request::new(Method::GET, "/events".parse().unwrap());
        present
            .headers_mut()
            .insert("last-event-id", HeaderValue::from_static("cursor-42"));
        assert_eq!(
            LastEventId::from_request(&present).unwrap().as_deref(),
            Some("cursor-42")
        );
        let mut invalid = Request::new(Method::GET, "/events".parse().unwrap());
        invalid.headers_mut().insert(
            "last-event-id",
            HeaderValue::from_bytes(&vec![b'x'; 1_025]).unwrap(),
        );
        assert!(LastEventId::from_request(&invalid).is_err());
    }
}
