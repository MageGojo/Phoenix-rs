//! Real gateways: `WeChat` Pay Native (`APIv3`) and Alipay Face-to-Face
//! (RSA2).
//!
//! Both providers speak to their gateway through the pluggable
//! [`PayHttp`] transport ([`HyperPayHttp`] by default), so tests can point
//! them at a local fake gateway. Every inbound byte is authenticated before
//! it is trusted:
//!
//! - `WeChat`: requests are signed RSA-SHA256 (PKCS#1 v1.5) with the merchant
//!   private key (`Authorization: WECHATPAY2-SHA256-RSA2048 ...`); responses
//!   and notifications are verified against the platform certificate selected
//!   by `Wechatpay-Serial` (downloaded from `GET /v3/certificates` and
//!   decrypted with the `APIv3` key, or preloaded from
//!   `platform_cert_path`); notify resources are AES-256-GCM decrypted.
//! - Alipay: request parameters are RSA2-signed (sorted `k=v&...`);
//!   synchronous responses are verified over the exact
//!   `alipay_trade_*_response` substring; asynchronous notifications are
//!   verified over the sorted form parameters minus `sign`/`sign_type`.
//!
//! Certificate caching: platform certificates live in an in-process cache
//! keyed by serial with a 12 h TTL; an unknown serial triggers one forced
//! refetch. Certificates loaded from `platform_cert_path` never expire in
//! process — rotate the file and restart (automatic file rotation is a
//! follow-up, see `docs/PAYMENTS.md`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use phoenix_http::{BoxFuture, Bytes, Method, StatusCode};

use crate::alipay;
use crate::crypto::{RsaSigner, RsaVerifier, gmt8_datetime, random_nonce, unix_timestamp};
use crate::transport::{GatewayRequest, GatewayResponse, HyperPayHttp, PayHttp, path_and_query};
use crate::wechat::{self, PlatformCerts, SignatureHeaders, request_message, response_message};
use crate::{
    AlipayF2FConfig, Amount, Bill, CreateOrder, Currency, NotifyEvent, NotifyRequest, PayError,
    PaymentAction, PaymentIntent, PaymentProvider, PaymentStatus, RefundNotifyEvent, RefundOrder,
    RefundReceipt, RefundStatus, WechatNativeConfig, parse_bill_csv, parse_bill_csv_bytes,
};

const USER_AGENT: &str = concat!("phoenix-pay/", env!("CARGO_PKG_VERSION"));

/// Upper bound on the inflated size of a downloaded bill archive.
///
/// A day of trades is megabytes at most; this is what stops a hostile or
/// corrupt archive from being expanded until the process dies.
const MAX_BILL_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;

/// Exhaustive match: adding a currency forces the gateways to handle it.
fn require_cny(order: &CreateOrder) {
    match order.amount.currency() {
        Currency::Cny => {}
    }
}

// ---------------------------------------------------------------------------
// WeChat Pay Native
// ---------------------------------------------------------------------------

struct WechatInner {
    config: WechatNativeConfig,
    http: Arc<dyn PayHttp>,
    base_url: String,
    signer: OnceLock<Result<Arc<RsaSigner>, PayError>>,
    certs: Mutex<Option<PlatformCerts>>,
}

/// `WeChat` Pay Native (扫码) provider over `APIv3`.
pub struct WechatNativeProvider {
    inner: Arc<WechatInner>,
}

impl std::fmt::Debug for WechatNativeProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WechatNativeProvider")
            .field("config", &self.inner.config)
            .field("base_url", &self.inner.base_url)
            .finish_non_exhaustive()
    }
}

impl WechatNativeProvider {
    /// Provider key registered by [`PaymentProvider::key`].
    pub const KEY: &'static str = "wechat_native";

    /// Bind the channel configuration with the default transport and the
    /// production gateway. Key material is loaded lazily on first use.
    #[must_use]
    pub fn new(config: WechatNativeConfig) -> Self {
        Self::with_transport(
            config,
            Arc::new(HyperPayHttp::new()),
            wechat::DEFAULT_BASE_URL,
        )
    }

    /// Bind with a custom transport and base URL (tests / sandbox / proxies).
    #[must_use]
    pub fn with_transport(
        config: WechatNativeConfig,
        http: Arc<dyn PayHttp>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(WechatInner {
                config,
                http,
                base_url: base_url.into().trim_end_matches('/').to_owned(),
                signer: OnceLock::new(),
                certs: Mutex::new(None),
            }),
        }
    }

    /// The bound configuration.
    #[must_use]
    pub fn config(&self) -> &WechatNativeConfig {
        &self.inner.config
    }

    /// Close an order on the gateway (`POST .../out-trade-no/{no}/close`).
    ///
    /// # Errors
    ///
    /// Returns [`PayError::Gateway`] on transport / gateway failures and
    /// [`PayError::OrderNotFound`] when the gateway does not know the order.
    #[must_use = "futures do nothing unless awaited"]
    pub fn close_order(&self, out_trade_no: &str) -> BoxFuture<Result<(), PayError>> {
        let inner = Arc::clone(&self.inner);
        let out_trade_no = out_trade_no.to_owned();
        Box::pin(async move { inner.close(&out_trade_no).await })
    }
}

impl WechatInner {
    fn signer(&self) -> Result<Arc<RsaSigner>, PayError> {
        self.signer
            .get_or_init(|| {
                let pem =
                    std::fs::read_to_string(&self.config.private_key_path).map_err(|error| {
                        PayError::Config(format!(
                            "read WeChat merchant private key {}: {error}",
                            self.config.private_key_path.display()
                        ))
                    })?;
                RsaSigner::from_pem(&pem).map(Arc::new)
            })
            .clone()
    }

    fn api_v3_key(&self) -> Result<Vec<u8>, PayError> {
        let key = self.config.api_v3_key.expose().as_bytes().to_vec();
        if key.len() == 32 {
            Ok(key)
        } else {
            Err(PayError::Config(format!(
                "WeChat APIv3 key must be 32 bytes, got {}",
                key.len()
            )))
        }
    }

    /// Send one signed `APIv3` request (does NOT verify the response).
    async fn signed_request(
        &self,
        method: Method,
        path: &str,
        body: String,
    ) -> Result<GatewayResponse, PayError> {
        let signer = self.signer()?;
        let url = format!("{}{path}", self.base_url);
        let canonical_path = path_and_query(&url)?;
        let timestamp = unix_timestamp();
        let nonce = random_nonce()?;
        let message = request_message(&method, &canonical_path, timestamp, &nonce, &body);
        let signature = signer.sign_base64(message.as_bytes())?;
        let authorization = wechat::authorization_header(
            &self.config.mch_id,
            &self.config.mch_serial_no,
            &nonce,
            timestamp,
            &signature,
        );
        let mut headers = vec![
            ("authorization", authorization),
            ("accept", "application/json".to_owned()),
            ("user-agent", USER_AGENT.to_owned()),
        ];
        if !body.is_empty() {
            headers.push(("content-type", "application/json".to_owned()));
        }
        self.http
            .request(GatewayRequest {
                method,
                url,
                headers,
                body: Bytes::from(body),
            })
            .await
    }

    /// Verify a response's `Wechatpay-*` signature headers, refetching the
    /// platform certificates once when the serial is unknown.
    async fn verify_response(&self, response: &GatewayResponse) -> Result<(), PayError> {
        let headers = SignatureHeaders::from_headers(&response.headers)
            .map_err(|error| PayError::Gateway(format!("unsigned gateway response: {error}")))?;
        headers
            .check_freshness(unix_timestamp())
            .map_err(|error| PayError::Gateway(error.to_string()))?;
        let message = response_message(&headers.timestamp, &headers.nonce, &response.body);
        for force in [false, true] {
            self.ensure_certs(force).await?;
            let verified = {
                let cache = self.lock_certs();
                cache
                    .as_ref()
                    .and_then(|certs| certs.get(&headers.serial))
                    .map(|verifier| verifier.verify(&message, &headers.signature))
            };
            match verified {
                Some(true) => return Ok(()),
                Some(false) => {
                    return Err(PayError::Gateway(
                        "WeChat response signature verification failed".to_owned(),
                    ));
                }
                None => {}
            }
        }
        Err(PayError::Gateway(format!(
            "no platform certificate matches serial `{}`",
            headers.serial
        )))
    }

    /// Verify a notification's signature headers and return the raw body for
    /// decryption. Same certificate handling as [`Self::verify_response`],
    /// but failures map to [`PayError::InvalidNotify`].
    async fn verify_notify_signature(&self, notify: &NotifyRequest) -> Result<(), PayError> {
        let headers = SignatureHeaders::from_headers(notify.headers())?;
        headers.check_freshness(unix_timestamp())?;
        let message = response_message(&headers.timestamp, &headers.nonce, notify.body());
        for force in [false, true] {
            self.ensure_certs(force).await?;
            let verified = {
                let cache = self.lock_certs();
                cache
                    .as_ref()
                    .and_then(|certs| certs.get(&headers.serial))
                    .map(|verifier| verifier.verify(&message, &headers.signature))
            };
            match verified {
                Some(true) => return Ok(()),
                Some(false) => {
                    return Err(PayError::InvalidNotify(
                        "WeChat notify signature verification failed".to_owned(),
                    ));
                }
                None => {}
            }
        }
        Err(PayError::InvalidNotify(format!(
            "no platform certificate matches Wechatpay-Serial `{}`",
            headers.serial
        )))
    }

    fn lock_certs(&self) -> std::sync::MutexGuard<'_, Option<PlatformCerts>> {
        self.certs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Make sure the platform certificate cache is populated and fresh.
    async fn ensure_certs(&self, force: bool) -> Result<(), PayError> {
        let now = unix_timestamp();
        {
            let cache = self.lock_certs();
            if let Some(certs) = cache.as_ref()
                && !force
                && certs.is_fresh(now)
            {
                return Ok(());
            }
        }
        let certs = if let Some(path) = &self.config.platform_cert_path {
            let pem = std::fs::read_to_string(path).map_err(|error| {
                PayError::Config(format!(
                    "read WeChat platform certificate {}: {error}",
                    path.display()
                ))
            })?;
            let (verifier, serial) = RsaVerifier::from_x509_pem(&pem)?;
            PlatformCerts::new(HashMap::from([(serial, verifier)]), now, true)
        } else {
            self.download_certs(now).await?
        };
        *self.lock_certs() = Some(certs);
        Ok(())
    }

    /// `GET /v3/certificates`, decrypt each certificate, and verify the
    /// response signature against the freshly decrypted set (bootstrap).
    async fn download_certs(&self, now: u64) -> Result<PlatformCerts, PayError> {
        let api_v3_key = self.api_v3_key()?;
        let response = self
            .signed_request(Method::GET, "/v3/certificates", String::new())
            .await?;
        if response.status != StatusCode::OK {
            return Err(PayError::Gateway(format!(
                "GET /v3/certificates returned {}",
                response.status
            )));
        }
        let body: wechat::CertificatesBody = serde_json::from_slice(&response.body)
            .map_err(|error| PayError::Gateway(format!("certificates body: {error}")))?;
        let mut verifiers = HashMap::new();
        for entry in &body.data {
            let der_or_pem = entry
                .encrypt_certificate
                .decrypt(&api_v3_key)
                .map_err(|error| {
                    PayError::Gateway(format!("decrypt platform certificate: {error}"))
                })?;
            let pem = String::from_utf8(der_or_pem).map_err(|_| {
                PayError::Gateway("platform certificate is not UTF-8 PEM".to_owned())
            })?;
            let (verifier, _cert_serial) = RsaVerifier::from_x509_pem(&pem)?;
            verifiers.insert(entry.serial_no.to_ascii_uppercase(), verifier);
        }
        let certs = PlatformCerts::new(verifiers, now, false);

        // Bootstrap verification: the certificates response itself must be
        // signed by one of the certificates it delivered.
        let headers = SignatureHeaders::from_headers(&response.headers).map_err(|error| {
            PayError::Gateway(format!("unsigned certificates response: {error}"))
        })?;
        headers
            .check_freshness(now)
            .map_err(|error| PayError::Gateway(error.to_string()))?;
        let message = response_message(&headers.timestamp, &headers.nonce, &response.body);
        let valid = certs
            .get(&headers.serial)
            .is_some_and(|verifier| verifier.verify(&message, &headers.signature));
        if !valid {
            return Err(PayError::Gateway(
                "certificates response signature verification failed".to_owned(),
            ));
        }
        Ok(certs)
    }

    async fn create(&self, order: CreateOrder) -> Result<PaymentIntent, PayError> {
        #[derive(serde::Deserialize)]
        struct NativeResponse {
            code_url: String,
        }
        order.validate()?;
        require_cny(&order);
        let body = serde_json::json!({
            "appid": self.config.app_id,
            "mchid": self.config.mch_id,
            "description": order.subject,
            "out_trade_no": order.out_trade_no,
            "notify_url": self.config.notify_url,
            "amount": { "total": order.amount.minor(), "currency": order.amount.currency().code() },
        })
        .to_string();
        let response = self
            .signed_request(Method::POST, "/v3/pay/transactions/native", body)
            .await?;
        self.verify_response(&response).await?;
        if response.status != StatusCode::OK {
            return Err(gateway_error("create native transaction", &response));
        }
        let parsed: NativeResponse = serde_json::from_slice(&response.body)
            .map_err(|error| PayError::Gateway(format!("native transaction body: {error}")))?;
        Ok(PaymentIntent {
            provider: WechatNativeProvider::KEY.to_owned(),
            out_trade_no: order.out_trade_no,
            amount: order.amount,
            action: PaymentAction::QrCode(parsed.code_url),
        })
    }

    async fn query(&self, out_trade_no: &str) -> Result<PaymentStatus, PayError> {
        let path = format!(
            "/v3/pay/transactions/out-trade-no/{out_trade_no}?mchid={}",
            self.config.mch_id
        );
        let response = self
            .signed_request(Method::GET, &path, String::new())
            .await?;
        self.verify_response(&response).await?;
        if response.status == StatusCode::NOT_FOUND {
            return Err(PayError::OrderNotFound {
                provider: WechatNativeProvider::KEY.to_owned(),
                out_trade_no: out_trade_no.to_owned(),
            });
        }
        if response.status != StatusCode::OK {
            return Err(gateway_error("query transaction", &response));
        }
        let resource: wechat::TransactionResource = serde_json::from_slice(&response.body)
            .map_err(|error| PayError::Gateway(format!("query transaction body: {error}")))?;
        wechat::map_trade_state(&resource.trade_state)
    }

    async fn close(&self, out_trade_no: &str) -> Result<(), PayError> {
        let path = format!("/v3/pay/transactions/out-trade-no/{out_trade_no}/close");
        let body = serde_json::json!({ "mchid": self.config.mch_id }).to_string();
        let response = self.signed_request(Method::POST, &path, body).await?;
        self.verify_response(&response).await?;
        match response.status {
            StatusCode::NO_CONTENT | StatusCode::OK => Ok(()),
            StatusCode::NOT_FOUND => Err(PayError::OrderNotFound {
                provider: WechatNativeProvider::KEY.to_owned(),
                out_trade_no: out_trade_no.to_owned(),
            }),
            _ => Err(gateway_error("close transaction", &response)),
        }
    }

    async fn refund(&self, refund: &RefundOrder) -> Result<RefundReceipt, PayError> {
        refund.validate()?;
        let mut body = serde_json::json!({
            "out_trade_no": refund.out_trade_no,
            "out_refund_no": refund.out_refund_no,
            "amount": {
                "refund": refund.amount.minor(),
                "total": refund.total.minor(),
                "currency": refund.amount.currency().code(),
            },
        });
        if let Some(reason) = &refund.reason {
            body["reason"] = serde_json::Value::String(reason.clone());
        }
        // The refund callback URL is per-request and separate from the
        // payment one; without it WeChat reports the outcome only on query.
        if let Some(url) = self
            .config
            .refund_notify_url
            .as_deref()
            .filter(|url| !url.is_empty())
        {
            body["notify_url"] = serde_json::Value::String(url.to_owned());
        }
        let response = self
            .signed_request(
                Method::POST,
                "/v3/refund/domestic/refunds",
                body.to_string(),
            )
            .await?;
        self.verify_response(&response).await?;
        if response.status != StatusCode::OK {
            return Err(gateway_error("create refund", &response));
        }
        Self::refund_receipt(&refund.out_trade_no, &response.body)
    }

    async fn query_refund(
        &self,
        out_trade_no: &str,
        out_refund_no: &str,
    ) -> Result<RefundReceipt, PayError> {
        let path = format!("/v3/refund/domestic/refunds/{out_refund_no}");
        let response = self
            .signed_request(Method::GET, &path, String::new())
            .await?;
        self.verify_response(&response).await?;
        if response.status == StatusCode::NOT_FOUND {
            return Err(PayError::RefundNotFound {
                provider: WechatNativeProvider::KEY.to_owned(),
                out_refund_no: out_refund_no.to_owned(),
            });
        }
        if response.status != StatusCode::OK {
            return Err(gateway_error("query refund", &response));
        }
        Self::refund_receipt(out_trade_no, &response.body)
    }

    /// Parse a refund response body into a normalized receipt.
    fn refund_receipt(out_trade_no: &str, body: &[u8]) -> Result<RefundReceipt, PayError> {
        let parsed: wechat::RefundResource = serde_json::from_slice(body)
            .map_err(|error| PayError::Gateway(format!("refund body: {error}")))?;
        Ok(RefundReceipt {
            provider: WechatNativeProvider::KEY.to_owned(),
            out_trade_no: if parsed.out_trade_no.is_empty() {
                out_trade_no.to_owned()
            } else {
                parsed.out_trade_no.clone()
            },
            out_refund_no: parsed.out_refund_no.clone(),
            refund_id: parsed.refund_id.clone(),
            amount: Amount::from_minor(parsed.amount.refund, Currency::Cny),
            status: wechat::map_refund_status(&parsed.status)?,
            raw: String::from_utf8_lossy(body).into_owned(),
        })
    }

    /// Two-step bill download: ask for a signed download URL, then fetch the
    /// CSV and check the digest the gateway published for it.
    async fn download_bill(&self, date: &str) -> Result<Bill, PayError> {
        #[derive(serde::Deserialize)]
        struct BillTicket {
            hash_type: String,
            hash_value: String,
            download_url: String,
        }

        let path = format!("/v3/bill/tradebill?bill_date={date}&bill_type=SUCCESS");
        let response = self
            .signed_request(Method::GET, &path, String::new())
            .await?;
        self.verify_response(&response).await?;
        if response.status != StatusCode::OK {
            return Err(gateway_error("request trade bill", &response));
        }
        let ticket: BillTicket = serde_json::from_slice(&response.body)
            .map_err(|error| PayError::Reconcile(format!("bill ticket: {error}")))?;

        let canonical = path_and_query(&ticket.download_url)?;
        let file = self
            .signed_get_absolute(&ticket.download_url, &canonical)
            .await?;
        if file.status != StatusCode::OK {
            return Err(PayError::Reconcile(format!(
                "bill download returned HTTP {}",
                file.status
            )));
        }
        // The file itself carries no signature headers, so the digest the
        // signed ticket published is the only integrity check available —
        // verify it before parsing a single row.
        verify_bill_digest(&ticket.hash_type, &ticket.hash_value, &file.body)?;

        let csv = std::str::from_utf8(&file.body)
            .map_err(|_| PayError::Reconcile("bill file is not UTF-8".to_owned()))?;
        parse_bill_csv(WechatNativeProvider::KEY, date, csv)
    }

    /// Signed GET against an absolute URL (the bill download host differs from
    /// the API base URL, so [`Self::signed_request`] cannot be reused).
    async fn signed_get_absolute(
        &self,
        url: &str,
        canonical_path: &str,
    ) -> Result<GatewayResponse, PayError> {
        let signer = self.signer()?;
        let timestamp = unix_timestamp();
        let nonce = random_nonce()?;
        let message = request_message(&Method::GET, canonical_path, timestamp, &nonce, "");
        let signature = signer.sign_base64(message.as_bytes())?;
        let authorization = wechat::authorization_header(
            &self.config.mch_id,
            &self.config.mch_serial_no,
            &nonce,
            timestamp,
            &signature,
        );
        self.http
            .request(GatewayRequest {
                method: Method::GET,
                url: url.to_owned(),
                headers: vec![
                    ("authorization", authorization),
                    ("accept", "text/csv".to_owned()),
                    ("user-agent", USER_AGENT.to_owned()),
                ],
                body: Bytes::new(),
            })
            .await
    }

    async fn verify_refund_notify(
        &self,
        notify: NotifyRequest,
    ) -> Result<RefundNotifyEvent, PayError> {
        // 1. Authenticate before parsing anything.
        self.verify_notify_signature(&notify).await?;
        let body: wechat::NotifyBody = serde_json::from_slice(notify.body())
            .map_err(|error| PayError::InvalidNotify(format!("notify body: {error}")))?;
        // 2. Refuse a payment callback delivered to the refund route: the two
        // resources have different shapes, and confusing them would apply a
        // payment event to a refund record.
        if !body.event_type.starts_with("REFUND.") {
            return Err(PayError::InvalidNotify(format!(
                "expected a REFUND.* event, got `{}`",
                body.event_type
            )));
        }
        let api_v3_key = self.api_v3_key()?;
        let plaintext = body
            .resource
            .decrypt(&api_v3_key)
            .map_err(|error| PayError::InvalidNotify(format!("notify resource: {error}")))?;
        let raw = String::from_utf8(plaintext)
            .map_err(|_| PayError::InvalidNotify("notify resource is not UTF-8".to_owned()))?;
        let resource: wechat::RefundNotifyResource = serde_json::from_str(&raw)
            .map_err(|error| PayError::InvalidNotify(format!("notify resource: {error}")))?;
        Ok(RefundNotifyEvent {
            out_trade_no: resource.out_trade_no,
            out_refund_no: resource.out_refund_no,
            refund_id: resource.refund_id,
            amount: Amount::from_minor(resource.amount.refund, Currency::Cny),
            status: wechat::map_refund_notify_status(&resource.refund_status)?,
            raw,
        })
    }

    async fn verify_notify(&self, notify: NotifyRequest) -> Result<NotifyEvent, PayError> {
        // 1. Authenticate: signature headers against the platform certificate.
        self.verify_notify_signature(&notify).await?;
        // 2. Only now parse and decrypt the payload.
        let body: wechat::NotifyBody = serde_json::from_slice(notify.body())
            .map_err(|error| PayError::InvalidNotify(format!("notify body: {error}")))?;
        let api_v3_key = self.api_v3_key()?;
        let plaintext = body
            .resource
            .decrypt(&api_v3_key)
            .map_err(|error| PayError::InvalidNotify(format!("notify resource: {error}")))?;
        let raw = String::from_utf8(plaintext)
            .map_err(|_| PayError::InvalidNotify("notify resource is not UTF-8".to_owned()))?;
        let resource: wechat::TransactionResource = serde_json::from_str(&raw)
            .map_err(|error| PayError::InvalidNotify(format!("notify resource: {error}")))?;
        Ok(NotifyEvent {
            out_trade_no: resource.out_trade_no,
            transaction_id: resource.transaction_id,
            status: wechat::map_trade_state(&resource.trade_state)?,
            raw,
        })
    }
}

fn gateway_error(operation: &str, response: &GatewayResponse) -> PayError {
    let detail: wechat::ErrorBody =
        serde_json::from_slice(&response.body).unwrap_or(wechat::ErrorBody {
            code: String::new(),
            message: String::new(),
        });
    PayError::Gateway(format!(
        "{operation} returned {}: code={} message={}",
        response.status, detail.code, detail.message
    ))
}

impl PaymentProvider for WechatNativeProvider {
    fn key(&self) -> &'static str {
        Self::KEY
    }

    fn create(&self, order: &CreateOrder) -> BoxFuture<Result<PaymentIntent, PayError>> {
        let inner = Arc::clone(&self.inner);
        let order = order.clone();
        Box::pin(async move { inner.create(order).await })
    }

    fn verify_notify(&self, notify: &NotifyRequest) -> BoxFuture<Result<NotifyEvent, PayError>> {
        let inner = Arc::clone(&self.inner);
        let notify = notify.clone();
        Box::pin(async move { inner.verify_notify(notify).await })
    }

    fn query(&self, out_trade_no: &str) -> BoxFuture<Result<PaymentStatus, PayError>> {
        let inner = Arc::clone(&self.inner);
        let out_trade_no = out_trade_no.to_owned();
        Box::pin(async move { inner.query(&out_trade_no).await })
    }

    fn close(&self, out_trade_no: &str) -> BoxFuture<Result<(), PayError>> {
        self.close_order(out_trade_no)
    }

    fn refund(&self, refund: &RefundOrder) -> BoxFuture<Result<RefundReceipt, PayError>> {
        let inner = Arc::clone(&self.inner);
        let refund = refund.clone();
        Box::pin(async move { inner.refund(&refund).await })
    }

    fn verify_refund_notify(
        &self,
        notify: &NotifyRequest,
    ) -> BoxFuture<Result<RefundNotifyEvent, PayError>> {
        let inner = Arc::clone(&self.inner);
        let notify = notify.clone();
        Box::pin(async move { inner.verify_refund_notify(notify).await })
    }

    fn query_refund(
        &self,
        out_trade_no: &str,
        out_refund_no: &str,
    ) -> BoxFuture<Result<RefundReceipt, PayError>> {
        let inner = Arc::clone(&self.inner);
        let out_trade_no = out_trade_no.to_owned();
        let out_refund_no = out_refund_no.to_owned();
        Box::pin(async move { inner.query_refund(&out_trade_no, &out_refund_no).await })
    }

    fn download_bill(&self, date: &str) -> BoxFuture<Result<Bill, PayError>> {
        let inner = Arc::clone(&self.inner);
        let date = date.to_owned();
        Box::pin(async move { inner.download_bill(&date).await })
    }
}

/// Check a downloaded bill against the digest the signed ticket published.
///
/// `WeChat` publishes `SHA1` for trade bills. An unknown algorithm is an error,
/// not a skipped check: silently trusting an unverified file is exactly what
/// this guard exists to prevent.
fn verify_bill_digest(hash_type: &str, expected: &str, body: &[u8]) -> Result<(), PayError> {
    if !hash_type.eq_ignore_ascii_case("SHA1") {
        return Err(PayError::Reconcile(format!(
            "unsupported bill hash type `{hash_type}`"
        )));
    }
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, body);
    let actual: String = digest.as_ref().iter().fold(
        String::with_capacity(digest.as_ref().len() * 2),
        |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        },
    );
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(PayError::Reconcile(
            "bill file digest does not match the signed ticket".to_owned(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Alipay Face-to-Face (当面付)
// ---------------------------------------------------------------------------

struct AlipayInner {
    config: AlipayF2FConfig,
    http: Arc<dyn PayHttp>,
    signer: OnceLock<Result<Arc<RsaSigner>, PayError>>,
    verifier: OnceLock<Result<Arc<RsaVerifier>, PayError>>,
}

/// Alipay 当面付 (F2F precreate) provider over the `OpenAPI` gateway.
pub struct AlipayF2FProvider {
    inner: Arc<AlipayInner>,
}

impl std::fmt::Debug for AlipayF2FProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AlipayF2FProvider")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl AlipayF2FProvider {
    /// Provider key registered by [`PaymentProvider::key`].
    pub const KEY: &'static str = "alipay_f2f";

    /// Bind the channel configuration with the default transport. The
    /// gateway URL comes from `config.gateway_url` (override for sandbox).
    /// Key material is parsed lazily on first use.
    #[must_use]
    pub fn new(config: AlipayF2FConfig) -> Self {
        Self::with_transport(config, Arc::new(HyperPayHttp::new()))
    }

    /// Bind with a custom transport (tests / instrumentation).
    #[must_use]
    pub fn with_transport(config: AlipayF2FConfig, http: Arc<dyn PayHttp>) -> Self {
        Self {
            inner: Arc::new(AlipayInner {
                config,
                http,
                signer: OnceLock::new(),
                verifier: OnceLock::new(),
            }),
        }
    }

    /// The bound configuration.
    #[must_use]
    pub fn config(&self) -> &AlipayF2FConfig {
        &self.inner.config
    }

    /// Close an order on the gateway (`alipay.trade.close`).
    ///
    /// # Errors
    ///
    /// Returns [`PayError::Gateway`] on transport / gateway failures and
    /// [`PayError::OrderNotFound`] when the gateway does not know the order.
    #[must_use = "futures do nothing unless awaited"]
    pub fn close_order(&self, out_trade_no: &str) -> BoxFuture<Result<(), PayError>> {
        let inner = Arc::clone(&self.inner);
        let out_trade_no = out_trade_no.to_owned();
        Box::pin(async move { inner.close(&out_trade_no).await })
    }

    /// Signed URL of the daily trade bill for `date` (`YYYY-MM-DD`).
    ///
    /// [`PaymentProvider::download_bill`] already fetches, unzips, and parses
    /// this; reach for the raw URL only to archive the original file or to
    /// hand it to an external pipeline.
    ///
    /// The URL is short-lived and grants access to settlement data — treat it
    /// as a credential and do not log it.
    #[must_use]
    pub fn bill_url(&self, date: &str) -> BoxFuture<Result<String, PayError>> {
        let inner = Arc::clone(&self.inner);
        let date = date.to_owned();
        Box::pin(async move { inner.bill_download_url(&date).await })
    }
}

impl AlipayInner {
    fn signer(&self) -> Result<Arc<RsaSigner>, PayError> {
        self.signer
            .get_or_init(|| {
                if self.config.sign_type != "RSA2" {
                    return Err(PayError::Config(format!(
                        "unsupported Alipay sign_type `{}` (only RSA2)",
                        self.config.sign_type
                    )));
                }
                RsaSigner::from_pem(self.config.app_private_key.expose()).map(Arc::new)
            })
            .clone()
    }

    fn verifier(&self) -> Result<Arc<RsaVerifier>, PayError> {
        self.verifier
            .get_or_init(|| {
                RsaVerifier::from_public_key_pem(self.config.alipay_public_key.expose())
                    .map(Arc::new)
            })
            .clone()
    }

    /// Call one `OpenAPI` method: sign, POST as a form, verify the response
    /// signature over the exact response-object substring, parse the payload.
    async fn call(
        &self,
        method: &str,
        biz_content: &serde_json::Value,
        with_notify_url: bool,
    ) -> Result<alipay::ResponsePayload, PayError> {
        let signer = self.signer()?;
        let verifier = self.verifier()?;

        let mut params = std::collections::BTreeMap::new();
        params.insert("app_id".to_owned(), self.config.app_id.clone());
        params.insert("method".to_owned(), method.to_owned());
        params.insert("format".to_owned(), "JSON".to_owned());
        params.insert("charset".to_owned(), "utf-8".to_owned());
        params.insert("sign_type".to_owned(), self.config.sign_type.clone());
        params.insert("timestamp".to_owned(), gmt8_datetime(unix_timestamp()));
        params.insert("version".to_owned(), "1.0".to_owned());
        if with_notify_url {
            params.insert("notify_url".to_owned(), self.config.notify_url.clone());
        }
        params.insert("biz_content".to_owned(), biz_content.to_string());
        let sign = signer.sign_base64(alipay::request_sign_content(&params).as_bytes())?;
        params.insert("sign".to_owned(), sign);
        let form = serde_urlencoded::to_string(&params)
            .map_err(|error| PayError::Gateway(format!("encode request form: {error}")))?;

        let response = self
            .http
            .request(GatewayRequest {
                method: Method::POST,
                url: self.config.gateway_url.clone(),
                headers: vec![
                    (
                        "content-type",
                        "application/x-www-form-urlencoded;charset=utf-8".to_owned(),
                    ),
                    ("user-agent", USER_AGENT.to_owned()),
                ],
                body: Bytes::from(form),
            })
            .await?;
        if response.status != StatusCode::OK {
            return Err(PayError::Gateway(format!(
                "{method} returned HTTP {}",
                response.status
            )));
        }
        let body = std::str::from_utf8(&response.body)
            .map_err(|_| PayError::Gateway("gateway response is not UTF-8".to_owned()))?;

        let response_key = format!("{}_response", method.replace('.', "_"));
        let content = alipay::extract_response_content(body, &response_key)
            .ok_or_else(|| PayError::Gateway(format!("missing `{response_key}` in response")))?;

        // Verify BEFORE trusting any field of the content.
        let envelope: serde_json::Value = serde_json::from_str(body)
            .map_err(|error| PayError::Gateway(format!("response envelope: {error}")))?;
        let sign = envelope
            .get("sign")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| PayError::Gateway("unsigned Alipay response".to_owned()))?;
        let signature = crate::crypto::b64_decode(sign)
            .map_err(|error| PayError::Gateway(format!("response sign: {error}")))?;
        if !verifier.verify(content.as_bytes(), &signature) {
            return Err(PayError::Gateway(
                "Alipay response signature verification failed".to_owned(),
            ));
        }

        serde_json::from_str(content)
            .map_err(|error| PayError::Gateway(format!("response payload: {error}")))
    }

    async fn create(&self, order: CreateOrder) -> Result<PaymentIntent, PayError> {
        order.validate()?;
        require_cny(&order);
        let biz_content = serde_json::json!({
            "out_trade_no": order.out_trade_no,
            "total_amount": order.amount.decimal_string(),
            "subject": order.subject,
        });
        let payload = self
            .call("alipay.trade.precreate", &biz_content, true)
            .await?;
        if !payload.is_success() {
            return Err(PayError::Gateway(
                payload.error_text("alipay.trade.precreate"),
            ));
        }
        let qr_code = payload.qr_code.ok_or_else(|| {
            PayError::Gateway("alipay.trade.precreate succeeded without qr_code".to_owned())
        })?;
        Ok(PaymentIntent {
            provider: AlipayF2FProvider::KEY.to_owned(),
            out_trade_no: order.out_trade_no,
            amount: order.amount,
            action: PaymentAction::QrCode(qr_code),
        })
    }

    async fn query(&self, out_trade_no: &str) -> Result<PaymentStatus, PayError> {
        let biz_content = serde_json::json!({ "out_trade_no": out_trade_no });
        let payload = self.call("alipay.trade.query", &biz_content, false).await?;
        if payload.sub_code == "ACQ.TRADE_NOT_EXIST" {
            return Err(PayError::OrderNotFound {
                provider: AlipayF2FProvider::KEY.to_owned(),
                out_trade_no: out_trade_no.to_owned(),
            });
        }
        if !payload.is_success() {
            return Err(PayError::Gateway(payload.error_text("alipay.trade.query")));
        }
        let trade_status = payload.trade_status.ok_or_else(|| {
            PayError::Gateway("alipay.trade.query succeeded without trade_status".to_owned())
        })?;
        alipay::map_trade_status(&trade_status)
    }

    async fn close(&self, out_trade_no: &str) -> Result<(), PayError> {
        let biz_content = serde_json::json!({ "out_trade_no": out_trade_no });
        let payload = self.call("alipay.trade.close", &biz_content, false).await?;
        if payload.sub_code == "ACQ.TRADE_NOT_EXIST" {
            return Err(PayError::OrderNotFound {
                provider: AlipayF2FProvider::KEY.to_owned(),
                out_trade_no: out_trade_no.to_owned(),
            });
        }
        if !payload.is_success() {
            return Err(PayError::Gateway(payload.error_text("alipay.trade.close")));
        }
        Ok(())
    }

    async fn refund(&self, refund: &RefundOrder) -> Result<RefundReceipt, PayError> {
        refund.validate()?;
        let mut biz_content = serde_json::json!({
            "out_trade_no": refund.out_trade_no,
            "refund_amount": refund.amount.decimal_string(),
            // Alipay's per-refund idempotency key. Required for partial
            // refunds and harmless for full ones, so it is always sent.
            "out_request_no": refund.out_refund_no,
        });
        if let Some(reason) = &refund.reason {
            biz_content["refund_reason"] = serde_json::Value::String(reason.clone());
        }
        let payload = self
            .call("alipay.trade.refund", &biz_content, false)
            .await?;
        if payload.sub_code == "ACQ.TRADE_NOT_EXIST" {
            return Err(PayError::OrderNotFound {
                provider: AlipayF2FProvider::KEY.to_owned(),
                out_trade_no: refund.out_trade_no.clone(),
            });
        }
        if !payload.is_success() {
            return Err(PayError::Gateway(payload.error_text("alipay.trade.refund")));
        }
        // A successful `alipay.trade.refund` means the money moved; the
        // asynchronous `Processing` state does not exist for this API.
        let amount = match &payload.refund_fee {
            Some(fee) => Amount::cny_from_decimal_str(fee)?,
            None => refund.amount,
        };
        Ok(RefundReceipt {
            provider: AlipayF2FProvider::KEY.to_owned(),
            out_trade_no: refund.out_trade_no.clone(),
            out_refund_no: refund.out_refund_no.clone(),
            refund_id: payload.trade_no.clone(),
            amount,
            status: RefundStatus::Succeeded,
            raw: String::new(),
        })
    }

    async fn query_refund(
        &self,
        out_trade_no: &str,
        out_refund_no: &str,
    ) -> Result<RefundReceipt, PayError> {
        let biz_content = serde_json::json!({
            "out_trade_no": out_trade_no,
            "out_request_no": out_refund_no,
        });
        let payload = self
            .call("alipay.trade.fastpay.refund.query", &biz_content, false)
            .await?;
        if payload.sub_code == "ACQ.TRADE_NOT_EXIST" {
            return Err(PayError::RefundNotFound {
                provider: AlipayF2FProvider::KEY.to_owned(),
                out_refund_no: out_refund_no.to_owned(),
            });
        }
        if !payload.is_success() {
            return Err(PayError::Gateway(
                payload.error_text("alipay.trade.fastpay.refund.query"),
            ));
        }
        // An empty `out_request_no` in a successful answer means Alipay has no
        // such refund: report it as missing rather than as a zero-amount one.
        if payload.out_request_no.is_none() && payload.refund_amount.is_none() {
            return Err(PayError::RefundNotFound {
                provider: AlipayF2FProvider::KEY.to_owned(),
                out_refund_no: out_refund_no.to_owned(),
            });
        }
        let amount = match &payload.refund_amount {
            Some(value) => Amount::cny_from_decimal_str(value)?,
            None => Amount::cny(0),
        };
        let status = match payload.refund_status.as_deref() {
            Some("REFUND_SUCCESS") | None => RefundStatus::Succeeded,
            Some(_) => RefundStatus::Processing,
        };
        Ok(RefundReceipt {
            provider: AlipayF2FProvider::KEY.to_owned(),
            out_trade_no: out_trade_no.to_owned(),
            out_refund_no: out_refund_no.to_owned(),
            refund_id: payload.trade_no.clone(),
            amount,
            status,
            raw: String::new(),
        })
    }

    /// Download and parse one day's bill.
    ///
    /// Alipay serves the bill as a ZIP holding a detail member and a summary
    /// member. Rather than guessing which is which from a filename in an
    /// unknown encoding, every member is offered to the parser and the one
    /// that yields the most rows wins — a member that is not a trade detail
    /// simply fails to match a header.
    async fn download_bill(&self, date: &str) -> Result<Bill, PayError> {
        let url = self.bill_download_url(date).await?;
        let response = self
            .http
            .request(GatewayRequest {
                method: Method::GET,
                url,
                headers: vec![("user-agent", USER_AGENT.to_owned())],
                body: Bytes::new(),
            })
            .await?;
        if response.status != StatusCode::OK {
            return Err(PayError::Reconcile(format!(
                "bill download returned HTTP {}",
                response.status
            )));
        }

        let members = crate::zip::read_entries(&response.body, MAX_BILL_ARCHIVE_BYTES)?;
        if members.is_empty() {
            return Err(PayError::Reconcile("bill archive is empty".to_owned()));
        }
        let mut names = Vec::with_capacity(members.len());
        let mut best: Option<Bill> = None;
        for member in &members {
            names.push(member.name.clone());
            let Ok(bill) = parse_bill_csv_bytes(AlipayF2FProvider::KEY, date, &member.data) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|current| bill.entries.len() > current.entries.len())
            {
                best = Some(bill);
            }
        }
        best.ok_or_else(|| {
            PayError::Reconcile(format!(
                "no member of the bill archive is a trade detail ({})",
                names.join(", ")
            ))
        })
    }

    /// Ask Alipay for the signed download URL of one day's bill.
    async fn bill_download_url(&self, date: &str) -> Result<String, PayError> {
        let biz_content = serde_json::json!({ "bill_type": "trade", "bill_date": date });
        let payload = self
            .call(
                "alipay.data.dataservice.bill.downloadurl.query",
                &biz_content,
                false,
            )
            .await?;
        if !payload.is_success() {
            return Err(PayError::Reconcile(
                payload.error_text("alipay.data.dataservice.bill.downloadurl.query"),
            ));
        }
        payload.bill_download_url.clone().ok_or_else(|| {
            PayError::Reconcile("bill download query succeeded without a URL".to_owned())
        })
    }

    fn verify_notify(&self, notify: &NotifyRequest) -> Result<NotifyEvent, PayError> {
        let verifier = self.verifier()?;
        let raw = notify.body_str()?.to_owned();
        let params: std::collections::BTreeMap<String, String> =
            serde_urlencoded::from_str(&raw)
                .map_err(|error| PayError::InvalidNotify(format!("notify form: {error}")))?;

        let sign_type = params.get("sign_type").map_or("", String::as_str);
        if sign_type != "RSA2" {
            return Err(PayError::InvalidNotify(format!(
                "unsupported notify sign_type `{sign_type}`"
            )));
        }
        let sign = params
            .get("sign")
            .ok_or_else(|| PayError::InvalidNotify("notify has no sign".to_owned()))?;
        let signature = crate::crypto::b64_decode(sign)
            .map_err(|error| PayError::InvalidNotify(format!("notify sign: {error}")))?;
        let content = alipay::notify_sign_content(&params);
        if !verifier.verify(content.as_bytes(), &signature) {
            return Err(PayError::InvalidNotify(
                "Alipay notify signature verification failed".to_owned(),
            ));
        }

        // Signature is valid — now (and only now) trust the fields.
        if params.get("app_id") != Some(&self.config.app_id) {
            return Err(PayError::InvalidNotify(
                "notify app_id does not match this channel".to_owned(),
            ));
        }
        let out_trade_no = params
            .get("out_trade_no")
            .cloned()
            .ok_or_else(|| PayError::InvalidNotify("notify has no out_trade_no".to_owned()))?;
        let trade_status = params
            .get("trade_status")
            .cloned()
            .ok_or_else(|| PayError::InvalidNotify("notify has no trade_status".to_owned()))?;
        Ok(NotifyEvent {
            out_trade_no,
            transaction_id: params.get("trade_no").cloned(),
            status: alipay::map_trade_status(&trade_status)?,
            raw,
        })
    }
}

impl PaymentProvider for AlipayF2FProvider {
    fn key(&self) -> &'static str {
        Self::KEY
    }

    fn create(&self, order: &CreateOrder) -> BoxFuture<Result<PaymentIntent, PayError>> {
        let inner = Arc::clone(&self.inner);
        let order = order.clone();
        Box::pin(async move { inner.create(order).await })
    }

    fn verify_notify(&self, notify: &NotifyRequest) -> BoxFuture<Result<NotifyEvent, PayError>> {
        let inner = Arc::clone(&self.inner);
        let notify = notify.clone();
        Box::pin(async move { inner.verify_notify(&notify) })
    }

    fn query(&self, out_trade_no: &str) -> BoxFuture<Result<PaymentStatus, PayError>> {
        let inner = Arc::clone(&self.inner);
        let out_trade_no = out_trade_no.to_owned();
        Box::pin(async move { inner.query(&out_trade_no).await })
    }

    fn close(&self, out_trade_no: &str) -> BoxFuture<Result<(), PayError>> {
        self.close_order(out_trade_no)
    }

    fn refund(&self, refund: &RefundOrder) -> BoxFuture<Result<RefundReceipt, PayError>> {
        let inner = Arc::clone(&self.inner);
        let refund = refund.clone();
        Box::pin(async move { inner.refund(&refund).await })
    }

    fn query_refund(
        &self,
        out_trade_no: &str,
        out_refund_no: &str,
    ) -> BoxFuture<Result<RefundReceipt, PayError>> {
        let inner = Arc::clone(&self.inner);
        let out_trade_no = out_trade_no.to_owned();
        let out_refund_no = out_refund_no.to_owned();
        Box::pin(async move { inner.query_refund(&out_trade_no, &out_refund_no).await })
    }

    fn download_bill(&self, date: &str) -> BoxFuture<Result<Bill, PayError>> {
        let inner = Arc::clone(&self.inner);
        let date = date.to_owned();
        Box::pin(async move { inner.download_bill(&date).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Amount;

    fn wechat() -> WechatNativeProvider {
        WechatNativeProvider::new(
            toml::from_str(
                r#"
                app_id = "wx1"
                mch_id = "m1"
                mch_serial_no = "s1"
                api_v3_key = "k"
                private_key_path = "does/not/exist.pem"
                notify_url = "https://example.com/pay/notify/wechat"
                "#,
            )
            .expect("config"),
        )
    }

    fn alipay() -> AlipayF2FProvider {
        AlipayF2FProvider::new(
            toml::from_str(
                r#"
                app_id = "a1"
                app_private_key = "not-a-key"
                alipay_public_key = "not-a-key"
                notify_url = "https://example.com/pay/notify/alipay"
                "#,
            )
            .expect("config"),
        )
    }

    #[test]
    fn keys_and_debug_redaction() {
        assert_eq!(wechat().key(), "wechat_native");
        assert_eq!(alipay().key(), "alipay_f2f");
        let debug = format!("{:?} {:?}", wechat(), alipay());
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("not-a-key"));
    }

    #[tokio::test]
    async fn broken_key_material_is_a_config_error() {
        let order = CreateOrder::new("T1", Amount::cny(100), "tea");
        assert!(matches!(
            wechat().create(&order).await,
            Err(PayError::Config(_))
        ));
        assert!(matches!(
            alipay().create(&order).await,
            Err(PayError::Config(_))
        ));
        assert!(matches!(
            alipay().query("T1").await,
            Err(PayError::Config(_))
        ));
    }

    #[tokio::test]
    async fn unsigned_notifications_are_rejected_without_io() {
        // WeChat: missing Wechatpay-* headers must fail before any network
        // or key access. Alipay: bad form / missing sign must fail before
        // any field is trusted (the broken key errors first here, which is
        // still a rejection).
        let notify = NotifyRequest::from_body("{}");
        assert!(matches!(
            wechat().verify_notify(&notify).await,
            Err(PayError::InvalidNotify(_))
        ));
        assert!(alipay().verify_notify(&notify).await.is_err());
    }

    #[tokio::test]
    async fn wechat_rejects_wrong_api_v3_key_length() {
        // 1-byte APIv3 key: certificate download must refuse to start.
        let provider = wechat();
        let inner = Arc::clone(&provider.inner);
        assert!(matches!(inner.api_v3_key(), Err(PayError::Config(_))));
    }
}
