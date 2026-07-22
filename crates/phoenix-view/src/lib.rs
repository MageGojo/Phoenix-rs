use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::stream;
use phoenix_http::{
    Bytes, HeaderValue, IntoResponse, Request, Response, RouteManifest, StatusCode, header,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    fmt::Write as _,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

mod assets;
mod renderer;

pub use assets::{
    ASSET_MANIFEST_SCHEMA, AssetEntry, AssetManifest, AssetManifestError, RendererManifest,
};
pub use renderer::{
    NodeRenderer, RenderContext, RenderFrame, RenderResult, RendererConfig, RendererError,
    RendererHealth, RendererStream,
};

const PAGE_MEDIA_TYPE: &str = "application/vnd.phoenix.page+json";
const PAGE_REQUEST_HEADER: &str = "x-phoenix-page";
const ENVELOPE_PURPOSE: &str = "page-navigation";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    Spa,
    Ssr,
    #[default]
    Islands,
}

impl RenderMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spa => "spa",
            Self::Ssr => "ssr",
            Self::Islands => "islands",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Island {
    pub id: String,
    pub component: String,
    pub props: Value,
}

impl Island {
    #[must_use]
    pub fn new(component: impl Into<String>, props: impl Serialize) -> Self {
        let component = component.into();
        Self {
            id: component.clone(),
            component,
            props: serde_json::to_value(props).unwrap_or(Value::Null),
        }
    }

    #[must_use]
    pub fn with_id(
        id: impl Into<String>,
        component: impl Into<String>,
        props: impl Serialize,
    ) -> Self {
        Self {
            id: id.into(),
            component: component.into(),
            props: serde_json::to_value(props).unwrap_or(Value::Null),
        }
    }
}

/// Controlled document metadata shared by full documents and page navigation.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PageHead {
    pub title: Option<String>,
    pub description: Option<String>,
    pub canonical: Option<String>,
    pub robots: Option<String>,
    pub open_graph: Option<OpenGraph>,
}

impl PageHead {
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub fn canonical(mut self, canonical: impl Into<String>) -> Self {
        self.canonical = Some(canonical.into());
        self
    }

    #[must_use]
    pub fn robots(mut self, robots: impl Into<String>) -> Self {
        self.robots = Some(robots.into());
        self
    }

    #[must_use]
    pub fn open_graph(mut self, open_graph: OpenGraph) -> Self {
        self.open_graph = Some(open_graph);
        self
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct OpenGraph {
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub kind: Option<String>,
}

impl OpenGraph {
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.image = Some(image.into());
        self
    }

    #[must_use]
    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PageEnvelope {
    pub protocol: u8,
    pub render_mode: RenderMode,
    pub page: String,
    pub props: Value,
    pub shared: Map<String, Value>,
    pub errors: Map<String, Value>,
    pub flash: Map<String, Value>,
    pub contract_hash: Option<String>,
    pub asset_version: Option<String>,
    pub request_id: Option<String>,
    #[serde(default)]
    pub head: PageHead,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csrf_token: Option<String>,
    pub routes: HashMap<String, String>,
    pub islands: Vec<Island>,
}

#[cfg(test)]
impl PageEnvelope {
    fn new_for_test(props: Value) -> Self {
        Self {
            protocol: 1,
            render_mode: RenderMode::Ssr,
            page: "test/page".to_owned(),
            props,
            shared: Map::new(),
            errors: Map::new(),
            flash: Map::new(),
            contract_hash: None,
            asset_version: None,
            request_id: None,
            head: PageHead::default(),
            csrf_token: None,
            routes: HashMap::new(),
            islands: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    envelope: PageEnvelope,
    server_html: Option<String>,
    script_src: String,
    stylesheets: Vec<String>,
}

impl Page {
    #[must_use]
    pub fn new(name: impl Into<String>, props: impl Serialize) -> Self {
        Self {
            envelope: PageEnvelope {
                protocol: 1,
                render_mode: RenderMode::default(),
                page: name.into(),
                props: serde_json::to_value(props).unwrap_or(Value::Null),
                shared: Map::new(),
                errors: Map::new(),
                flash: Map::new(),
                contract_hash: None,
                asset_version: None,
                request_id: None,
                head: PageHead::default(),
                csrf_token: None,
                routes: HashMap::new(),
                islands: Vec::new(),
            },
            server_html: None,
            script_src: default_script_src(),
            stylesheets: Vec::new(),
        }
    }

    #[must_use]
    pub fn mode(mut self, mode: RenderMode) -> Self {
        self.envelope.render_mode = mode;
        self
    }

    #[must_use]
    pub fn spa(self) -> Self {
        self.mode(RenderMode::Spa)
    }

    #[must_use]
    pub fn ssr(self) -> Self {
        self.mode(RenderMode::Ssr)
    }

    #[must_use]
    pub fn islands(self) -> Self {
        self.mode(RenderMode::Islands)
    }

    #[must_use]
    pub fn shared(mut self, shared: impl Serialize) -> Self {
        self.envelope.shared = object_value(shared);
        self
    }

    #[must_use]
    pub fn errors(mut self, errors: impl Serialize) -> Self {
        self.envelope.errors = object_value(errors);
        self
    }

    #[must_use]
    pub fn flash(mut self, flash: impl Serialize) -> Self {
        self.envelope.flash = object_value(flash);
        self
    }

    #[must_use]
    pub fn contract_hash(mut self, hash: impl Into<String>) -> Self {
        self.envelope.contract_hash = Some(hash.into());
        self
    }

    #[must_use]
    pub fn asset_version(mut self, version: impl Into<String>) -> Self {
        self.envelope.asset_version = Some(version.into());
        self
    }

    #[must_use]
    pub fn request_id(mut self, request_id: impl Into<String>) -> Self {
        self.envelope.request_id = Some(request_id.into());
        self
    }

    #[must_use]
    pub fn head(mut self, head: PageHead) -> Self {
        self.envelope.head = head;
        self
    }

    #[must_use]
    pub fn csrf_token(mut self, csrf_token: impl Into<String>) -> Self {
        self.envelope.csrf_token = Some(csrf_token.into());
        self
    }

    #[must_use]
    pub fn island(mut self, component: impl Into<String>, props: impl Serialize) -> Self {
        self.envelope.islands.push(Island::new(component, props));
        self
    }

    #[must_use]
    pub fn island_with_id(
        mut self,
        id: impl Into<String>,
        component: impl Into<String>,
        props: impl Serialize,
    ) -> Self {
        self.envelope
            .islands
            .push(Island::with_id(id, component, props));
        self
    }

    /// Attach HTML produced by a trusted React server renderer.
    #[must_use]
    pub fn trusted_server_html(mut self, html: impl Into<String>) -> Self {
        self.server_html = Some(html.into());
        self
    }

    /// Apply HTML and island descriptors returned by the trusted SSR renderer.
    #[must_use]
    pub fn rendered(mut self, result: RenderResult) -> Self {
        self.server_html = Some(result.html);
        self.envelope.islands = result.islands;
        self
    }

    /// Apply one entry from a validated production asset manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is missing or references an undeclared
    /// asset.
    pub fn production_assets(
        mut self,
        manifest: &AssetManifest,
        entry_name: &str,
    ) -> Result<Self, AssetManifestError> {
        manifest.validate()?;
        let entry = manifest
            .entry(entry_name)
            .ok_or_else(|| AssetManifestError::UnknownAsset(entry_name.to_owned()))?;
        self.script_src = manifest.url(&entry.file)?;
        self.stylesheets = entry
            .css
            .iter()
            .map(|asset| manifest.url(asset))
            .collect::<Result<Vec<_>, _>>()?;
        self.envelope.contract_hash = Some(manifest.contract_hash.clone());
        self.envelope.asset_version = Some(manifest.version.clone());
        Ok(self)
    }

    /// Render an SSR or Islands page and select the document or page-protocol response.
    pub async fn respond_with_renderer(
        self,
        request: &Request,
        renderer: &NodeRenderer,
    ) -> Response {
        let context = RenderContext::new(request.uri().to_string());
        match renderer.render(self.envelope(), &context).await {
            Ok(result) => self
                .rendered(result)
                .respond_to(request, None)
                .into_response(),
            Err(error) => {
                eprintln!("SSR renderer failed: {error}");
                Response::text("SSR renderer unavailable")
                    .with_status(StatusCode::SERVICE_UNAVAILABLE)
            }
        }
    }

    /// Stream an SSR/Islands document as renderer chunks arrive. Page-protocol
    /// navigation requests still return the normal atomic JSON envelope.
    pub fn respond_streaming_with_renderer(
        mut self,
        request: &Request,
        renderer: &NodeRenderer,
    ) -> Response {
        if let Some(manifest) = request.extensions().get::<RouteManifest>() {
            self.envelope.routes.clone_from(manifest.routes());
        }
        if Self::is_page_request(request.headers()) {
            return self
                .respond(true, None)
                .unwrap_or_else(PageResponseError::into_response);
        }

        let context = RenderContext::new(request.uri().to_string());
        let Ok(mut frame_stream) = renderer.render_stream(&self.envelope, &context) else {
            return Response::text("SSR renderer unavailable")
                .with_status(StatusCode::SERVICE_UNAVAILABLE);
        };
        let prefix = document_prefix(&self.envelope, &self.stylesheets);
        let render_mode = self.envelope.render_mode;
        let script_src = self.script_src;
        let mut envelope = self.envelope;
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            if sender.send(Bytes::from(prefix)).await.is_err() {
                return;
            }
            let mut completed = false;
            while let Some(frame) = frame_stream.recv().await {
                match frame {
                    Ok(RenderFrame::Chunk(chunk)) => {
                        if sender.send(Bytes::from(chunk)).await.is_err() {
                            return;
                        }
                    }
                    Ok(RenderFrame::Complete { islands, .. }) => {
                        envelope.islands = islands;
                        completed = true;
                        break;
                    }
                    Err(_) => break,
                }
            }
            if !completed {
                let _ = sender
                    .send(Bytes::from_static(
                        b"<!-- Phoenix SSR stream interrupted -->",
                    ))
                    .await;
            }
            if let Ok(suffix) = document_suffix(&envelope, &script_src) {
                let _ = sender.send(Bytes::from(suffix)).await;
            }
        });
        let body = stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|chunk| (chunk, receiver))
        });
        let mut response = Response::stream(body);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        response.headers_mut().insert(
            "x-phoenix-render-mode",
            HeaderValue::from_static(render_mode.as_str()),
        );
        response
            .headers_mut()
            .insert("x-phoenix-ssr-stream", HeaderValue::from_static("1"));
        response
    }

    /// Override the browser entrypoint, for example when using a Vite dev server.
    #[must_use]
    pub fn script_src(mut self, source: impl Into<String>) -> Self {
        self.script_src = source.into();
        self
    }

    /// Select a document or protocol response from the incoming request.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or the selected codec fails.
    pub fn respond_to(
        mut self,
        request: &Request,
        codec: Option<&dyn PayloadCodec>,
    ) -> Result<Response, PageResponseError> {
        if let Some(manifest) = request.extensions().get::<RouteManifest>() {
            self.envelope.routes.clone_from(manifest.routes());
        }
        let page_request = Self::is_page_request(request.headers());
        self.respond(page_request, codec)
    }

    #[must_use]
    pub const fn envelope(&self) -> &PageEnvelope {
        &self.envelope
    }

    /// Return either a document response or a page-protocol response.
    ///
    /// Set `X-Phoenix-Page: 1` on client navigation requests. A codec encrypts
    /// only those protocol responses; initial HTML always contains readable
    /// hydration data because the browser must render it.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or the selected codec fails.
    pub fn respond(
        self,
        page_request: bool,
        codec: Option<&dyn PayloadCodec>,
    ) -> Result<Response, PageResponseError> {
        if page_request {
            return protocol_response(&self.envelope, codec);
        }

        document_response(
            &self.envelope,
            self.server_html.as_deref(),
            &self.script_src,
            &self.stylesheets,
        )
    }

    #[must_use]
    pub fn is_page_request(headers: &phoenix_http::HeaderMap) -> bool {
        headers
            .get(PAGE_REQUEST_HEADER)
            .is_some_and(|value| value == "1")
    }
}

fn default_script_src() -> String {
    if cfg!(debug_assertions) {
        let vite_url =
            std::env::var("VITE_DEV_URL").unwrap_or_else(|_| "http://127.0.0.1:5173".to_owned());
        return format!(
            "{}/@id/__x00__virtual:phoenix/client",
            vite_url.trim_end_matches('/')
        );
    }
    "/assets/phoenix.js".to_owned()
}

impl IntoResponse for Page {
    fn into_response(self) -> Response {
        self.respond(false, None)
            .unwrap_or_else(PageResponseError::into_response)
    }
}

pub trait PayloadCodec: Send + Sync {
    /// Encode serialized page JSON for a trusted client or intermediary.
    ///
    /// # Errors
    ///
    /// Returns an error if the payload cannot be encoded.
    fn encode(&self, plaintext: &[u8]) -> Result<EncryptedPayload, EncryptionError>;
}

#[derive(Clone)]
pub struct Aes256GcmCodec {
    key_id: String,
    cipher: Aes256Gcm,
    ttl: Duration,
}

impl Aes256GcmCodec {
    #[must_use]
    pub fn new(key_id: impl Into<String>, key: [u8; 32]) -> Self {
        Self {
            key_id: key_id.into(),
            cipher: Aes256Gcm::new(&key.into()),
            ttl: Duration::from_mins(1),
        }
    }

    #[must_use]
    pub const fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Decode a payload produced by this codec.
    ///
    /// # Errors
    ///
    /// Returns an error for the wrong key id, malformed base64, invalid nonce,
    /// or failed authentication.
    pub fn decode(&self, payload: &EncryptedPayload) -> Result<Vec<u8>, EncryptionError> {
        if payload.version != 1
            || payload.algorithm != "A256GCM"
            || payload.key_id != self.key_id
            || payload.purpose != ENVELOPE_PURPOSE
        {
            return Err(EncryptionError::InvalidEnvelope);
        }
        let now = unix_timestamp()?;
        if payload.expires_at < now || payload.issued_at > now.saturating_add(60) {
            return Err(EncryptionError::Expired);
        }
        let nonce = URL_SAFE_NO_PAD
            .decode(&payload.nonce)
            .map_err(|_| EncryptionError::InvalidEnvelope)?;
        let mut ciphertext = URL_SAFE_NO_PAD
            .decode(&payload.ciphertext)
            .map_err(|_| EncryptionError::InvalidEnvelope)?;
        let tag = URL_SAFE_NO_PAD
            .decode(&payload.tag)
            .map_err(|_| EncryptionError::InvalidEnvelope)?;
        if nonce.len() != 12 {
            return Err(EncryptionError::InvalidEnvelope);
        }
        ciphertext.extend(tag);
        let aad = envelope_aad(payload);

        self.cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| EncryptionError::AuthenticationFailed)
    }
}

impl PayloadCodec for Aes256GcmCodec {
    fn encode(&self, plaintext: &[u8]) -> Result<EncryptedPayload, EncryptionError> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let issued_at = unix_timestamp()?;
        let expires_at = issued_at.saturating_add(self.ttl.as_secs());
        let mut envelope = EncryptedPayload {
            version: 1,
            algorithm: "A256GCM".to_owned(),
            key_id: self.key_id.clone(),
            purpose: ENVELOPE_PURPOSE.to_owned(),
            issued_at,
            expires_at,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: String::new(),
            tag: String::new(),
        };
        let aad = envelope_aad(&envelope);
        let mut sealed = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| EncryptionError::EncryptionFailed)?;

        if sealed.len() < 16 {
            return Err(EncryptionError::EncryptionFailed);
        }
        let tag = sealed.split_off(sealed.len() - 16);
        envelope.ciphertext = URL_SAFE_NO_PAD.encode(sealed);
        envelope.tag = URL_SAFE_NO_PAD.encode(tag);
        Ok(envelope)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EncryptedPayload {
    pub version: u8,
    pub algorithm: String,
    pub key_id: String,
    pub purpose: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub nonce: String,
    pub ciphertext: String,
    pub tag: String,
}

#[derive(Debug, Error)]
pub enum EncryptionError {
    #[error("page payload encryption failed")]
    EncryptionFailed,
    #[error("the encrypted page envelope is invalid")]
    InvalidEnvelope,
    #[error("page payload authentication failed")]
    AuthenticationFailed,
    #[error("the encrypted page envelope has expired")]
    Expired,
    #[error("the system clock is before the Unix epoch")]
    InvalidClock,
}

#[derive(Debug, Error)]
pub enum PageResponseError {
    #[error("page serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Encryption(#[from] EncryptionError),
}

impl IntoResponse for PageResponseError {
    fn into_response(self) -> Response {
        Response::text("Internal Server Error").with_status(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

fn object_value(value: impl Serialize) -> Map<String, Value> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn unix_timestamp() -> Result<u64, EncryptionError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| EncryptionError::InvalidClock)
}

fn envelope_aad(payload: &EncryptedPayload) -> String {
    format!(
        "phoenix.page.v{}|{}|{}|{}|{}",
        payload.version, payload.key_id, payload.purpose, payload.issued_at, payload.expires_at
    )
}

fn protocol_response(
    envelope: &PageEnvelope,
    codec: Option<&dyn PayloadCodec>,
) -> Result<Response, PageResponseError> {
    let plain = serde_json::to_vec(envelope)?;
    let (body, encrypted) = match codec {
        Some(codec) => (serde_json::to_vec(&codec.encode(&plain)?)?, true),
        None => (plain, false),
    };
    let mut response = Response::new(StatusCode::OK, body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PAGE_MEDIA_TYPE),
    );
    response.headers_mut().insert(
        "x-phoenix-encrypted",
        HeaderValue::from_static(if encrypted { "1" } else { "0" }),
    );
    Ok(response)
}

fn document_response(
    envelope: &PageEnvelope,
    server_html: Option<&str>,
    script_src: &str,
    stylesheets: &[String],
) -> Result<Response, PageResponseError> {
    let root_html = if envelope.render_mode == RenderMode::Spa {
        ""
    } else {
        server_html.unwrap_or_default()
    };
    let html = format!(
        "{}{root_html}{}",
        document_prefix(envelope, stylesheets),
        document_suffix(envelope, script_src)?
    );
    let mut response = Response::new(StatusCode::OK, html);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response.headers_mut().insert(
        "x-phoenix-render-mode",
        HeaderValue::from_static(envelope.render_mode.as_str()),
    );
    Ok(response)
}

fn document_prefix(envelope: &PageEnvelope, stylesheets: &[String]) -> String {
    let styles = stylesheets
        .iter()
        .fold(String::new(), |mut styles, source| {
            let _ = write!(
                styles,
                "<link rel=\"stylesheet\" href=\"{}\">",
                html_attribute(source)
            );
            styles
        });
    let head = document_head(&envelope.head);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">{head}{styles}</head><body><div id=\"phoenix-root\" data-render-mode=\"{}\">",
        envelope.render_mode.as_str()
    )
}

fn document_head(head: &PageHead) -> String {
    let mut output = String::new();
    if let Some(title) = &head.title {
        let _ = write!(output, "<title>{}</title>", html_text(title));
    }
    if let Some(description) = &head.description {
        push_meta(&mut output, "name", "description", description);
    }
    if let Some(canonical) = &head.canonical {
        let _ = write!(
            output,
            "<link rel=\"canonical\" href=\"{}\">",
            html_attribute(canonical)
        );
    }
    if let Some(robots) = &head.robots {
        push_meta(&mut output, "name", "robots", robots);
    }
    if let Some(open_graph) = &head.open_graph {
        if let Some(value) = &open_graph.title {
            push_meta(&mut output, "property", "og:title", value);
        }
        if let Some(value) = &open_graph.description {
            push_meta(&mut output, "property", "og:description", value);
        }
        if let Some(value) = &open_graph.image {
            push_meta(&mut output, "property", "og:image", value);
        }
        if let Some(value) = &open_graph.kind {
            push_meta(&mut output, "property", "og:type", value);
        }
    }
    output
}

fn push_meta(output: &mut String, key: &str, name: &str, content: &str) {
    let _ = write!(
        output,
        "<meta {key}=\"{}\" content=\"{}\">",
        html_attribute(name),
        html_attribute(content)
    );
}

fn document_suffix(envelope: &PageEnvelope, script_src: &str) -> Result<String, PageResponseError> {
    let payload = json_for_html(envelope)?;
    let script_src = html_attribute(script_src);
    Ok(format!(
        "</div><script id=\"phoenix-page\" type=\"application/json\">{payload}</script><script type=\"module\" src=\"{script_src}\"></script></body></html>"
    ))
}

fn html_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn json_for_html(value: &impl Serialize) -> Result<String, serde_json::Error> {
    serde_json::to_string(value).map(|json| {
        json.replace('&', "\\u0026")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use phoenix_http::{Method, ResponseBody};
    use serde_json::json;
    use std::ffi::OsString;

    #[test]
    fn islands_is_the_default_and_html_payload_is_context_safe() {
        let response =
            Page::new("articles/show", json!({ "body": "</script><b>" })).into_response();
        let body = String::from_utf8_lossy(response.body());

        assert_eq!(
            response.headers().get("x-phoenix-render-mode").unwrap(),
            "islands"
        );
        assert!(body.contains("data-render-mode=\"islands\""));
        assert!(!body.contains("</script><b>"));
        assert!(body.contains("\\u003c/script\\u003e\\u003cb\\u003e"));
    }

    #[test]
    fn page_head_and_csrf_are_escaped_and_shared_by_the_protocol() {
        let head = PageHead::new("Apps < trusted")
            .description("Install \"safely\" & privately")
            .canonical("https://example.test/apps?kind=ios&view=all")
            .robots("index,follow")
            .open_graph(
                OpenGraph::new("Apps < trusted")
                    .description("No <script> here")
                    .image("https://example.test/cover.png?size=large&safe=1")
                    .kind("website"),
            );
        let page = Page::new("apps/index", json!({}))
            .head(head.clone())
            .csrf_token("csrf-token");
        let response = page.clone().into_response();
        let body = String::from_utf8_lossy(response.body());

        assert!(body.contains("<title>Apps &lt; trusted</title>"));
        assert!(body.contains("content=\"Install &quot;safely&quot; &amp; privately\""));
        assert!(body.contains("property=\"og:title\" content=\"Apps &lt; trusted\""));
        assert!(!body.contains("<script> here"));

        let protocol = page.respond(true, None).expect("protocol response");
        let envelope: PageEnvelope = serde_json::from_slice(protocol.body()).expect("envelope");
        assert_eq!(envelope.head, head);
        assert_eq!(envelope.csrf_token.as_deref(), Some("csrf-token"));
    }

    #[test]
    fn modes_share_the_same_page_protocol() {
        for mode in [RenderMode::Spa, RenderMode::Ssr, RenderMode::Islands] {
            let response = Page::new("articles/show", json!({ "id": 7 }))
                .mode(mode)
                .respond(true, None)
                .unwrap();
            let envelope: PageEnvelope = serde_json::from_slice(response.body()).unwrap();

            assert_eq!(envelope.render_mode, mode);
            assert_eq!(envelope.page, "articles/show");
            assert_eq!(envelope.props["id"], 7);
        }
    }

    #[test]
    fn spa_document_keeps_the_client_root_empty() {
        let response = Page::new("dashboard/show", json!({ "ready": true }))
            .spa()
            .trusted_server_html("<h1>server-only</h1>")
            .into_response();

        assert!(!String::from_utf8_lossy(response.body()).contains("server-only"));
    }

    #[test]
    fn custom_script_source_is_attribute_encoded() {
        let response = Page::new("dashboard/show", json!({}))
            .script_src("http://localhost/app.js?mode=dev&name=\"test\"")
            .into_response();
        let html = String::from_utf8_lossy(response.body());

        assert!(
            html.contains("src=\"http://localhost/app.js?mode=dev&amp;name=&quot;test&quot;\"")
        );
    }

    #[test]
    fn production_manifest_selects_hashed_scripts_styles_and_identity() {
        let manifest = AssetManifest {
            schema: ASSET_MANIFEST_SCHEMA,
            version: "sha256-build".to_owned(),
            contract_hash: "fnv1a-contract".to_owned(),
            public_path: "/assets/".to_owned(),
            entries: HashMap::from([(
                "client".to_owned(),
                AssetEntry {
                    file: "phoenix-a1.js".to_owned(),
                    css: vec!["client-b2.css".to_owned()],
                    imports: Vec::new(),
                },
            )]),
        };
        let response = Page::new("dashboard/show", json!({}))
            .production_assets(&manifest, "client")
            .unwrap()
            .into_response();
        let html = String::from_utf8_lossy(response.body());

        assert!(html.contains("src=\"/assets/phoenix-a1.js\""));
        assert!(html.contains("href=\"/assets/client-b2.css\""));
        assert!(html.contains("\"contract_hash\":\"fnv1a-contract\""));
        assert!(html.contains("\"asset_version\":\"sha256-build\""));
    }

    #[tokio::test]
    async fn page_response_forwards_renderer_chunks_before_hydration_payload() {
        let source = r"
          const readline = require('node:readline');
          readline.createInterface({input: process.stdin}).on('line', line => {
            const request = JSON.parse(line);
            if (request.kind === 'hello') {
              console.log(JSON.stringify({protocol: 1, id: request.id, ok: true})); return;
            }
            console.log(JSON.stringify({protocol: 1, id: request.id, ok: true, kind: 'chunk', chunk: '<h1>'}));
            console.log(JSON.stringify({protocol: 1, id: request.id, ok: true, kind: 'chunk', chunk: 'Streamed</h1>'}));
            console.log(JSON.stringify({protocol: 1, id: request.id, ok: true, kind: 'complete',
              islands: [{id: 'counter', component: 'counter', props: {value: 1}}]}));
          });
        ";
        let node = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join("node"))
                    .find(|candidate| candidate.is_file())
            })
            .expect("Node.js is required for streaming tests");
        let renderer = NodeRenderer::new(RendererConfig::command(
            node,
            [OsString::from("--eval"), OsString::from(source)],
        ));
        let request = Request::new(Method::GET, "/stream".parse().unwrap());
        let response = Page::new("test/page", json!({ "ready": true }))
            .ssr()
            .respond_streaming_with_renderer(&request, &renderer);
        assert!(response.is_streaming());
        assert_eq!(response.headers()["x-phoenix-ssr-stream"], "1");
        let (_, _, body) = response.into_parts();
        let ResponseBody::Stream(stream) = body else {
            panic!("expected streaming body");
        };
        let chunks = stream.collect::<Vec<_>>().await;
        let html = chunks
            .into_iter()
            .map(Result::unwrap)
            .fold(Vec::new(), |mut output, chunk| {
                output.extend_from_slice(&chunk);
                output
            });
        let html = String::from_utf8(html).unwrap();

        assert!(html.contains("<h1>Streamed</h1>"));
        assert!(html.contains("\"component\":\"counter\""));
        assert!(html.find("<h1>").unwrap() < html.find("phoenix-page").unwrap());
    }

    #[test]
    fn encrypted_protocol_payload_round_trips_and_rejects_wrong_keys() {
        let codec = Aes256GcmCodec::new("primary", [7; 32]);
        let response = Page::new("account/show", json!({ "balance": 42 }))
            .respond(true, Some(&codec))
            .unwrap();
        let encrypted: EncryptedPayload = serde_json::from_slice(response.body()).unwrap();
        let decoded = codec.decode(&encrypted).unwrap();
        let envelope: PageEnvelope = serde_json::from_slice(&decoded).unwrap();

        assert_eq!(response.headers().get("x-phoenix-encrypted").unwrap(), "1");
        assert_eq!(envelope.props["balance"], 42);
        assert!(
            Aes256GcmCodec::new("primary", [8; 32])
                .decode(&encrypted)
                .is_err()
        );
    }
}
