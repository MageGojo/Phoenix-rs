use std::{fmt::Write as _, sync::Arc};

use phoenix_http::CspNonce;
use serde::Serialize;
use thiserror::Error;

use crate::{PageEnvelope, PageHead};

type DocumentRender = dyn for<'a> Fn(DocumentContext<'a>) -> Result<DocumentSlots, DocumentTemplateError>
    + Send
    + Sync;

/// Request-scoped inputs available to an application document template.
#[derive(Clone, Copy, Debug)]
pub struct DocumentContext<'a> {
    envelope: &'a PageEnvelope,
    nonce: Option<&'a CspNonce>,
}

impl<'a> DocumentContext<'a> {
    pub(crate) const fn new(envelope: &'a PageEnvelope, nonce: Option<&'a CspNonce>) -> Self {
        Self { envelope, nonce }
    }

    #[must_use]
    pub const fn envelope(self) -> &'a PageEnvelope {
        self.envelope
    }

    #[must_use]
    pub const fn nonce(self) -> Option<&'a CspNonce> {
        self.nonce
    }

    /// Return an escaped nonce attribute for trusted custom script/style tags.
    #[must_use]
    pub fn nonce_attribute(self) -> String {
        nonce_attribute(self.nonce)
    }
}

/// HTML supplied by trusted application code for a document chrome slot.
///
/// Phoenix inserts this value verbatim. Build it only from application-owned
/// markup or content that has already been sanitized for its HTML context.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrustedHtml(String);

impl TrustedHtml {
    #[must_use]
    pub fn new(html: impl Into<String>) -> Self {
        Self(html.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Customizable slots around Phoenix's required React document elements.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocumentSlots {
    html_attributes: Vec<(String, String)>,
    body_attributes: Vec<(String, String)>,
    root_attributes: Vec<(String, String)>,
    head: TrustedHtml,
    before_root: TrustedHtml,
    after_root: TrustedHtml,
}

impl DocumentSlots {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the document language using an escaped `lang` attribute.
    #[must_use]
    pub fn language(mut self, language: impl Into<String>) -> Self {
        set_attribute(&mut self.html_attributes, "lang", language.into());
        self
    }

    /// Add or replace one escaped attribute on the `<html>` element.
    ///
    /// # Errors
    ///
    /// Returns an error when the attribute name is not valid HTML syntax.
    pub fn html_attribute(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, DocumentTemplateError> {
        let name = checked_attribute_name(name.into())?;
        set_attribute(&mut self.html_attributes, &name, value.into());
        Ok(self)
    }

    /// Add or replace one escaped attribute on the `<body>` element.
    ///
    /// # Errors
    ///
    /// Returns an error when the attribute name is not valid HTML syntax.
    pub fn body_attribute(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, DocumentTemplateError> {
        let name = checked_attribute_name(name.into())?;
        set_attribute(&mut self.body_attributes, &name, value.into());
        Ok(self)
    }

    /// Add or replace one escaped attribute on `#phoenix-root`.
    ///
    /// Phoenix owns `id` and `data-render-mode`, so templates cannot replace
    /// either attribute.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid or framework-owned attribute names.
    pub fn root_attribute(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, DocumentTemplateError> {
        let name = checked_attribute_name(name.into())?;
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "id" | "data-render-mode"
        ) {
            return Err(DocumentTemplateError::ReservedRootAttribute(name));
        }
        set_attribute(&mut self.root_attributes, &name, value.into());
        Ok(self)
    }

    /// Append trusted markup after framework metadata and styles in `<head>`.
    #[must_use]
    pub fn head(mut self, html: TrustedHtml) -> Self {
        self.head = html;
        self
    }

    /// Insert trusted markup immediately before `#phoenix-root`.
    #[must_use]
    pub fn before_root(mut self, html: TrustedHtml) -> Self {
        self.before_root = html;
        self
    }

    /// Insert trusted markup after `#phoenix-root` and before Phoenix scripts.
    #[must_use]
    pub fn after_root(mut self, html: TrustedHtml) -> Self {
        self.after_root = html;
        self
    }
}

/// Cloneable application hook for customizing the outer HTML document.
///
/// The callback controls document chrome only. Phoenix still owns the React
/// root, hydration payload, module entrypoint, escaping, and request CSP nonce.
#[derive(Clone)]
pub struct DocumentTemplate {
    render: Arc<DocumentRender>,
}

impl DocumentTemplate {
    /// Use one static set of document slots for every page.
    #[must_use]
    pub fn new(slots: DocumentSlots) -> Self {
        Self::from_fn(move |_| slots.clone())
    }

    /// Generate document slots from each page envelope.
    #[must_use]
    pub fn from_fn<F>(render: F) -> Self
    where
        F: for<'a> Fn(DocumentContext<'a>) -> DocumentSlots + Send + Sync + 'static,
    {
        Self {
            render: Arc::new(move |context| Ok(render(context))),
        }
    }

    /// Generate fallible document slots from each page envelope.
    #[must_use]
    pub fn try_from_fn<F>(render: F) -> Self
    where
        F: for<'a> Fn(DocumentContext<'a>) -> Result<DocumentSlots, DocumentTemplateError>
            + Send
            + Sync
            + 'static,
    {
        Self {
            render: Arc::new(render),
        }
    }

    pub(crate) fn render(
        &self,
        context: DocumentContext<'_>,
    ) -> Result<DocumentSlots, DocumentTemplateError> {
        (self.render)(context)
    }
}

impl Default for DocumentTemplate {
    fn default() -> Self {
        Self::new(DocumentSlots::default())
    }
}

impl std::fmt::Debug for DocumentTemplate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DocumentTemplate(<function>)")
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DocumentTemplateError {
    #[error("invalid HTML attribute name `{0}`")]
    InvalidAttributeName(String),
    #[error("Phoenix owns the root attribute `{0}`")]
    ReservedRootAttribute(String),
    #[error("document template failed: {0}")]
    Render(String),
}

impl DocumentTemplateError {
    #[must_use]
    pub fn render(message: impl Into<String>) -> Self {
        Self::Render(message.into())
    }
}

pub(crate) fn document_prefix(
    envelope: &PageEnvelope,
    stylesheets: &[String],
    nonce: Option<&CspNonce>,
    slots: &DocumentSlots,
) -> String {
    let nonce_attribute = nonce_attribute(nonce);
    let styles = stylesheets
        .iter()
        .fold(String::new(), |mut styles, source| {
            let _ = write!(
                styles,
                "<link rel=\"stylesheet\" href=\"{}\"{nonce_attribute}>",
                html_attribute(source),
            );
            styles
        });
    let head = document_head(&envelope.head);
    let nonce_meta = nonce.map_or_else(String::new, |_| {
        format!("<meta property=\"csp-nonce\"{nonce_attribute}>")
    });
    let html_attributes = render_attributes(&slots.html_attributes);
    let body_attributes = render_attributes(&slots.body_attributes);
    let root_attributes = render_attributes(&slots.root_attributes);
    format!(
        "<!doctype html><html{html_attributes}><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">{nonce_meta}{head}{styles}{custom_head}</head><body{body_attributes}>{before_root}<div id=\"phoenix-root\" data-render-mode=\"{mode}\"{root_attributes}>",
        custom_head = slots.head.as_str(),
        before_root = slots.before_root.as_str(),
        mode = envelope.render_mode.as_str(),
    )
}

pub(crate) fn document_suffix(
    envelope: &PageEnvelope,
    script_src: &str,
    nonce: Option<&CspNonce>,
    slots: &DocumentSlots,
) -> Result<String, serde_json::Error> {
    let payload = json_for_html(envelope)?;
    let script_src = html_attribute(script_src);
    let nonce_attribute = nonce_attribute(nonce);
    Ok(format!(
        "</div>{after_root}<script id=\"phoenix-page\" type=\"application/json\"{nonce_attribute}>{payload}</script><script type=\"module\" src=\"{script_src}\"{nonce_attribute}></script></body></html>",
        after_root = slots.after_root.as_str(),
    ))
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

fn checked_attribute_name(name: String) -> Result<String, DocumentTemplateError> {
    let mut characters = name.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || matches!(character, '_' | ':'));
    if !valid_start
        || !characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | ':' | '.' | '-')
        })
    {
        return Err(DocumentTemplateError::InvalidAttributeName(name));
    }
    Ok(name)
}

fn set_attribute(attributes: &mut Vec<(String, String)>, name: &str, value: String) {
    if let Some((_, current)) = attributes
        .iter_mut()
        .find(|(current, _)| current.eq_ignore_ascii_case(name))
    {
        *current = value;
    } else {
        attributes.push((name.to_owned(), value));
    }
}

fn render_attributes(attributes: &[(String, String)]) -> String {
    attributes
        .iter()
        .fold(String::new(), |mut output, (name, value)| {
            let _ = write!(output, " {name}=\"{}\"", html_attribute(value));
            output
        })
}

fn nonce_attribute(nonce: Option<&CspNonce>) -> String {
    nonce.map_or_else(String::new, |nonce| {
        format!(" nonce=\"{}\"", html_attribute(nonce.as_str()))
    })
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
    use serde_json::json;

    use super::*;
    use crate::PageEnvelope;

    #[test]
    fn custom_slots_escape_attributes_and_preserve_framework_elements() {
        let envelope = PageEnvelope::new_for_test(json!({ "safe": "</script>" }));
        let nonce = CspNonce::new("0123456789abcdef0123456789abcdef").unwrap();
        let slots = DocumentSlots::new()
            .language("zh-CN")
            .body_attribute("data-theme", "dark\" data-bad=\"1")
            .unwrap()
            .root_attribute("class", "application-root")
            .unwrap()
            .head(TrustedHtml::new(
                "<meta name=\"application\" content=\"custom\">",
            ))
            .before_root(TrustedHtml::new("<header>Brand</header>"))
            .after_root(TrustedHtml::new("<footer>Footer</footer>"));

        let prefix = document_prefix(
            &envelope,
            &["/assets/app.css".to_owned()],
            Some(&nonce),
            &slots,
        );
        let suffix = document_suffix(&envelope, "/assets/app.js", Some(&nonce), &slots).unwrap();

        assert!(prefix.contains("<html lang=\"zh-CN\">"));
        assert!(prefix.contains("<body data-theme=\"dark&quot; data-bad=&quot;1\">"));
        assert!(prefix.contains("<header>Brand</header><div id=\"phoenix-root\""));
        assert!(prefix.contains("class=\"application-root\""));
        assert!(prefix.contains("<meta name=\"application\" content=\"custom\">"));
        assert!(prefix.contains("nonce=\"0123456789abcdef0123456789abcdef\""));
        assert!(suffix.starts_with("</div><footer>Footer</footer>"));
        assert!(suffix.contains("\\u003c/script\\u003e"));
        assert!(suffix.contains("id=\"phoenix-page\""));
        assert!(suffix.contains("type=\"module\""));
    }

    #[test]
    fn template_functions_receive_page_and_nonce_context() {
        let envelope = PageEnvelope::new_for_test(json!({}));
        let nonce = CspNonce::new("fedcba9876543210fedcba9876543210").unwrap();
        let template = DocumentTemplate::from_fn(|context| {
            DocumentSlots::new().head(TrustedHtml::new(format!(
                "<script{}>window.page={:?}</script>",
                context.nonce_attribute(),
                context.envelope().page,
            )))
        });

        let slots = template
            .render(DocumentContext::new(&envelope, Some(&nonce)))
            .unwrap();
        let prefix = document_prefix(&envelope, &[], Some(&nonce), &slots);

        assert!(prefix.contains("window.page=\"test/page\""));
        assert!(prefix.contains("<script nonce=\"fedcba9876543210fedcba9876543210\">"));
    }

    #[test]
    fn invalid_and_framework_owned_attributes_fail_closed() {
        assert!(matches!(
            DocumentSlots::new().body_attribute("on load", "bad"),
            Err(DocumentTemplateError::InvalidAttributeName(_))
        ));
        assert!(matches!(
            DocumentSlots::new().root_attribute("ID", "replacement"),
            Err(DocumentTemplateError::ReservedRootAttribute(_))
        ));
    }
}
