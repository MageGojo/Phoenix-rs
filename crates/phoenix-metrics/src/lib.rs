//! Low-cardinality application metrics and Prometheus text export.

use std::{
    array,
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
    time::Instant,
};

use phoenix_http::{
    BoxFuture, IntoResponse, Method, Middleware, Next, Request, Response, ResponseBodyOutcome,
    ResponseBodySummary, StatusCode,
};

const METHODS: [&str; 9] = [
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "OTHER",
];
const STATUS_CLASSES: [&str; 6] = ["1xx", "2xx", "3xx", "4xx", "5xx", "other"];
const RESPONSE_OUTCOMES: [&str; 5] = [
    "complete",
    "stream_error",
    "delivery_cancelled",
    "shutdown",
    "aborted",
];
const DURATION_BUCKETS_MS: [u64; 11] = [5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000];

struct Inner {
    requests: [AtomicU64; METHODS.len() * STATUS_CLASSES.len()],
    request_duration_buckets: [AtomicU64; DURATION_BUCKETS_MS.len()],
    request_duration_count: AtomicU64,
    request_duration_micros: AtomicU64,
    active_requests: AtomicI64,
    response_terminations: [AtomicU64; RESPONSE_OUTCOMES.len()],
    connections_total: AtomicU64,
    active_connections: AtomicI64,
    tls_success: AtomicU64,
    tls_failure: AtomicU64,
    renderer: RendererAtomics,
    database_success: AtomicU64,
    database_failure: AtomicU64,
    queue_completed: AtomicU64,
    queue_failed: AtomicU64,
    queue_retried: AtomicU64,
    session_conflicts: AtomicU64,
    session_store_errors: AtomicU64,
    rate_limit_rejections: AtomicU64,
    rate_limit_store_errors: AtomicU64,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            requests: array::from_fn(|_| AtomicU64::new(0)),
            request_duration_buckets: array::from_fn(|_| AtomicU64::new(0)),
            request_duration_count: AtomicU64::new(0),
            request_duration_micros: AtomicU64::new(0),
            active_requests: AtomicI64::new(0),
            response_terminations: array::from_fn(|_| AtomicU64::new(0)),
            connections_total: AtomicU64::new(0),
            active_connections: AtomicI64::new(0),
            tls_success: AtomicU64::new(0),
            tls_failure: AtomicU64::new(0),
            renderer: RendererAtomics::default(),
            database_success: AtomicU64::new(0),
            database_failure: AtomicU64::new(0),
            queue_completed: AtomicU64::new(0),
            queue_failed: AtomicU64::new(0),
            queue_retried: AtomicU64::new(0),
            session_conflicts: AtomicU64::new(0),
            session_store_errors: AtomicU64::new(0),
            rate_limit_rejections: AtomicU64::new(0),
            rate_limit_store_errors: AtomicU64::new(0),
        }
    }
}

#[derive(Default)]
struct RendererAtomics {
    ready_workers: AtomicU64,
    active_requests: AtomicU64,
    rendered_requests: AtomicU64,
    failures: AtomicU64,
    restarts: AtomicU64,
    timeouts: AtomicU64,
}

/// A cloneable, process-local metrics registry with a fixed label vocabulary.
#[derive(Clone, Default)]
pub struct Metrics(Arc<Inner>);

impl std::fmt::Debug for Metrics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Metrics").finish_non_exhaustive()
    }
}

impl Metrics {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start tracking one accepted connection. Dropping the guard decrements the active gauge.
    #[must_use]
    pub fn connection_opened(&self) -> ConnectionGuard {
        self.0.connections_total.fetch_add(1, Ordering::Relaxed);
        self.0.active_connections.fetch_add(1, Ordering::Relaxed);
        ConnectionGuard(Some(self.clone()))
    }

    pub fn record_tls_handshake(&self, success: bool) {
        let counter = if success {
            &self.0.tls_success
        } else {
            &self.0.tls_failure
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_renderer(&self, snapshot: RendererMetricsSnapshot) {
        self.0
            .renderer
            .ready_workers
            .store(snapshot.ready_workers, Ordering::Relaxed);
        self.0
            .renderer
            .active_requests
            .store(snapshot.active_requests, Ordering::Relaxed);
        self.0
            .renderer
            .rendered_requests
            .store(snapshot.rendered_requests, Ordering::Relaxed);
        self.0
            .renderer
            .failures
            .store(snapshot.failures, Ordering::Relaxed);
        self.0
            .renderer
            .restarts
            .store(snapshot.restarts, Ordering::Relaxed);
        self.0
            .renderer
            .timeouts
            .store(snapshot.timeouts, Ordering::Relaxed);
    }

    pub fn record_database(&self, outcome: DatabaseOutcome) {
        match outcome {
            DatabaseOutcome::Success => &self.0.database_success,
            DatabaseOutcome::Failure => &self.0.database_failure,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_job(&self, outcome: JobOutcome) {
        match outcome {
            JobOutcome::Completed => &self.0.queue_completed,
            JobOutcome::Failed => &self.0.queue_failed,
            JobOutcome::Retried => &self.0.queue_retried,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_session_conflict(&self) {
        self.0.session_conflicts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_session_store_error(&self) {
        self.0.session_store_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rate_limit_rejection(&self) {
        self.0.rate_limit_rejections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rate_limit_store_error(&self) {
        self.0
            .rate_limit_store_errors
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Render the Prometheus 0.0.4 text exposition format.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn render(&self) -> String {
        let mut output = String::with_capacity(4_096);
        output.push_str("# HELP phoenix_http_requests_total Completed HTTP requests.\n");
        output.push_str("# TYPE phoenix_http_requests_total counter\n");
        for (method_index, method) in METHODS.iter().enumerate() {
            for (status_index, status) in STATUS_CLASSES.iter().enumerate() {
                let value = self.0.requests[method_index * STATUS_CLASSES.len() + status_index]
                    .load(Ordering::Relaxed);
                if value != 0 {
                    writeln!(
                        output,
                        "phoenix_http_requests_total{{method=\"{method}\",status_class=\"{status}\"}} {value}"
                    )
                    .expect("writing to a String cannot fail");
                }
            }
        }
        gauge(
            &mut output,
            "phoenix_http_active_requests",
            self.0.active_requests.load(Ordering::Relaxed),
        );
        output.push_str("# TYPE phoenix_http_request_duration_seconds histogram\n");
        let mut cumulative = 0_u64;
        for (index, bucket) in DURATION_BUCKETS_MS.iter().enumerate() {
            cumulative = cumulative
                .saturating_add(self.0.request_duration_buckets[index].load(Ordering::Relaxed));
            writeln!(
                output,
                "phoenix_http_request_duration_seconds_bucket{{le=\"{}.{:03}\"}} {cumulative}",
                bucket / 1_000,
                bucket % 1_000
            )
            .expect("writing to a String cannot fail");
        }
        let count = self.0.request_duration_count.load(Ordering::Relaxed);
        writeln!(
            output,
            "phoenix_http_request_duration_seconds_bucket{{le=\"+Inf\"}} {count}"
        )
        .expect("writing to a String cannot fail");
        let micros = self.0.request_duration_micros.load(Ordering::Relaxed);
        writeln!(
            output,
            "phoenix_http_request_duration_seconds_sum {}.{:06}",
            micros / 1_000_000,
            micros % 1_000_000
        )
        .expect("writing to a String cannot fail");
        writeln!(
            output,
            "phoenix_http_request_duration_seconds_count {count}"
        )
        .expect("writing to a String cannot fail");
        output.push_str(
            "# HELP phoenix_http_response_terminations_total Final response body outcomes.\n",
        );
        output.push_str("# TYPE phoenix_http_response_terminations_total counter\n");
        for (index, outcome) in RESPONSE_OUTCOMES.iter().enumerate() {
            labeled_counter(
                &mut output,
                "phoenix_http_response_terminations_total",
                "outcome",
                outcome,
                self.0.response_terminations[index].load(Ordering::Relaxed),
            );
        }
        counter(
            &mut output,
            "phoenix_connections_total",
            self.0.connections_total.load(Ordering::Relaxed),
        );
        gauge(
            &mut output,
            "phoenix_connections_active",
            self.0.active_connections.load(Ordering::Relaxed),
        );
        labeled_counter(
            &mut output,
            "phoenix_tls_handshakes_total",
            "outcome",
            "success",
            self.0.tls_success.load(Ordering::Relaxed),
        );
        labeled_counter(
            &mut output,
            "phoenix_tls_handshakes_total",
            "outcome",
            "failure",
            self.0.tls_failure.load(Ordering::Relaxed),
        );
        renderer_metrics(&mut output, &self.0.renderer);
        labeled_counter(
            &mut output,
            "phoenix_database_operations_total",
            "outcome",
            "success",
            self.0.database_success.load(Ordering::Relaxed),
        );
        labeled_counter(
            &mut output,
            "phoenix_database_operations_total",
            "outcome",
            "failure",
            self.0.database_failure.load(Ordering::Relaxed),
        );
        labeled_counter(
            &mut output,
            "phoenix_queue_jobs_total",
            "outcome",
            "completed",
            self.0.queue_completed.load(Ordering::Relaxed),
        );
        labeled_counter(
            &mut output,
            "phoenix_queue_jobs_total",
            "outcome",
            "failed",
            self.0.queue_failed.load(Ordering::Relaxed),
        );
        labeled_counter(
            &mut output,
            "phoenix_queue_jobs_total",
            "outcome",
            "retried",
            self.0.queue_retried.load(Ordering::Relaxed),
        );
        counter(
            &mut output,
            "phoenix_session_conflicts_total",
            self.0.session_conflicts.load(Ordering::Relaxed),
        );
        counter(
            &mut output,
            "phoenix_session_store_errors_total",
            self.0.session_store_errors.load(Ordering::Relaxed),
        );
        counter(
            &mut output,
            "phoenix_rate_limit_rejections_total",
            self.0.rate_limit_rejections.load(Ordering::Relaxed),
        );
        counter(
            &mut output,
            "phoenix_rate_limit_store_errors_total",
            self.0.rate_limit_store_errors.load(Ordering::Relaxed),
        );
        output
    }

    #[must_use]
    pub fn response(&self) -> Response {
        let mut response = self.render().into_response();
        response.headers_mut().insert(
            phoenix_http::header::CONTENT_TYPE,
            phoenix_http::HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        );
        response
    }

    fn request_started(&self) -> RequestGuard {
        self.0.active_requests.fetch_add(1, Ordering::Relaxed);
        RequestGuard {
            metrics: self.clone(),
            started: Instant::now(),
            active: true,
        }
    }
}

pub struct ConnectionGuard(Option<Metrics>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Some(metrics) = self.0.take() {
            metrics.0.active_connections.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

struct RequestGuard {
    metrics: Metrics,
    started: Instant,
    active: bool,
}

impl RequestGuard {
    fn complete(mut self, method: &Method, status: StatusCode, summary: ResponseBodySummary) {
        let method = method_index(method);
        let status = status_index(status);
        self.metrics.0.requests[method * STATUS_CLASSES.len() + status]
            .fetch_add(1, Ordering::Relaxed);
        let elapsed = self.started.elapsed();
        let millis = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        if let Some(index) = DURATION_BUCKETS_MS
            .iter()
            .position(|bucket| millis <= *bucket)
        {
            self.metrics.0.request_duration_buckets[index].fetch_add(1, Ordering::Relaxed);
        }
        self.metrics
            .0
            .request_duration_count
            .fetch_add(1, Ordering::Relaxed);
        self.metrics.0.request_duration_micros.fetch_add(
            u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.metrics.0.response_terminations[response_outcome_index(summary.outcome())]
            .fetch_add(1, Ordering::Relaxed);
        self.release();
    }

    fn release(&mut self) {
        if self.active {
            self.active = false;
            self.metrics
                .0
                .active_requests
                .fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if self.active {
            self.metrics.0.response_terminations
                [response_outcome_index(ResponseBodyOutcome::Aborted)]
            .fetch_add(1, Ordering::Relaxed);
            self.release();
        }
    }
}

#[derive(Clone, Debug)]
pub struct MetricsMiddleware {
    metrics: Metrics,
}

impl MetricsMiddleware {
    #[must_use]
    pub fn new(metrics: Metrics) -> Self {
        Self { metrics }
    }
}

impl Middleware for MetricsMiddleware {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        let metrics = self.metrics.clone();
        Box::pin(async move {
            let method = request.method().clone();
            let guard = metrics.request_started();
            let mut response = next.run(request).await;
            let status = response.status();
            response.on_body_finish(move |summary| {
                guard.complete(&method, status, summary);
            });
            response
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RendererMetricsSnapshot {
    pub ready_workers: u64,
    pub active_requests: u64,
    pub rendered_requests: u64,
    pub failures: u64,
    pub restarts: u64,
    pub timeouts: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseOutcome {
    Success,
    Failure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobOutcome {
    Completed,
    Failed,
    Retried,
}

fn method_index(method: &Method) -> usize {
    match *method {
        Method::GET => 0,
        Method::POST => 1,
        Method::PUT => 2,
        Method::PATCH => 3,
        Method::DELETE => 4,
        Method::HEAD => 5,
        Method::OPTIONS => 6,
        Method::CONNECT => 7,
        _ => 8,
    }
}

fn status_index(status: StatusCode) -> usize {
    match status.as_u16() / 100 {
        1 => 0,
        2 => 1,
        3 => 2,
        4 => 3,
        5 => 4,
        _ => 5,
    }
}

const fn response_outcome_index(outcome: ResponseBodyOutcome) -> usize {
    match outcome {
        ResponseBodyOutcome::Complete => 0,
        ResponseBodyOutcome::StreamError => 1,
        ResponseBodyOutcome::DeliveryCancelled => 2,
        ResponseBodyOutcome::Shutdown => 3,
        ResponseBodyOutcome::Aborted => 4,
    }
}

fn counter(output: &mut String, name: &str, value: u64) {
    writeln!(output, "# TYPE {name} counter\n{name} {value}")
        .expect("writing to a String cannot fail");
}

fn gauge(output: &mut String, name: &str, value: i64) {
    writeln!(output, "# TYPE {name} gauge\n{name} {value}")
        .expect("writing to a String cannot fail");
}

fn labeled_counter(output: &mut String, name: &str, label: &str, value: &str, count: u64) {
    writeln!(output, "{name}{{{label}=\"{value}\"}} {count}")
        .expect("writing to a String cannot fail");
}

fn renderer_metrics(output: &mut String, renderer: &RendererAtomics) {
    for (name, value) in [
        (
            "phoenix_renderer_ready_workers",
            renderer.ready_workers.load(Ordering::Relaxed),
        ),
        (
            "phoenix_renderer_active_requests",
            renderer.active_requests.load(Ordering::Relaxed),
        ),
        (
            "phoenix_renderer_rendered_requests_total",
            renderer.rendered_requests.load(Ordering::Relaxed),
        ),
        (
            "phoenix_renderer_failures_total",
            renderer.failures.load(Ordering::Relaxed),
        ),
        (
            "phoenix_renderer_restarts_total",
            renderer.restarts.load(Ordering::Relaxed),
        ),
        (
            "phoenix_renderer_timeouts_total",
            renderer.timeouts.load(Ordering::Relaxed),
        ),
    ] {
        writeln!(output, "{name} {value}").expect("writing to a String cannot fail");
    }
}

#[cfg(test)]
mod tests {
    use futures_util::{StreamExt, stream};
    use phoenix_http::{Bytes, ResponseBody};
    use phoenix_routing::Routes;

    use super::*;

    #[tokio::test]
    async fn middleware_and_exporter_use_only_bounded_labels() {
        let metrics = Metrics::new();
        let router = Routes::new()
            .get("/ok", |_request: Request| async { "ok" })
            .with_middleware(MetricsMiddleware::new(metrics.clone()))
            .build()
            .unwrap();
        let response = router
            .handle(Request::new(
                Method::GET,
                "/ok?secret=value".parse().unwrap(),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        metrics.record_tls_handshake(true);
        metrics.record_database(DatabaseOutcome::Failure);
        metrics.record_job(JobOutcome::Retried);
        let guard = metrics.connection_opened();
        let rendered = metrics.render();
        assert!(rendered.contains("method=\"GET\",status_class=\"2xx\"} 1"));
        assert!(rendered.contains("phoenix_connections_active 1"));
        assert!(rendered.contains("outcome=\"failure\"} 1"));
        assert!(!rendered.contains("secret"));
        drop(guard);
        assert!(metrics.render().contains("phoenix_connections_active 0"));
        assert_eq!(
            metrics.response().headers()[phoenix_http::header::CONTENT_TYPE],
            "text/plain; version=0.0.4; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn streaming_requests_remain_active_until_the_body_finishes() {
        let metrics = Metrics::new();
        let router = Routes::new()
            .get("/stream", |_request: Request| async {
                Response::stream(stream::iter([
                    Bytes::from_static(b"first"),
                    Bytes::from_static(b"second"),
                ]))
            })
            .with_middleware(MetricsMiddleware::new(metrics.clone()))
            .build()
            .unwrap();
        let response = router
            .handle(Request::new(Method::GET, "/stream".parse().unwrap()))
            .await;

        let before = metrics.render();
        assert!(before.contains("phoenix_http_active_requests 1"));
        assert!(!before.contains("method=\"GET\",status_class=\"2xx\"} 1"));
        let (_, _, body) = response.into_parts();
        let ResponseBody::Stream(stream) = body else {
            panic!("expected stream");
        };
        let chunks = stream.collect::<Vec<_>>().await;
        assert_eq!(chunks.len(), 2);

        let after = metrics.render();
        assert!(after.contains("phoenix_http_active_requests 0"));
        assert!(after.contains("method=\"GET\",status_class=\"2xx\"} 1"));
        assert!(after.contains("phoenix_http_response_terminations_total{outcome=\"complete\"} 1"));
    }
}
