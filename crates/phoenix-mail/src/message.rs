use crate::{
    MailError,
    address::{Address, contains_control_char, reject_header_injection},
};

/// Outbound email ready for a [`MailTransport`](crate::MailTransport).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    from: Address,
    to: Vec<Address>,
    cc: Vec<Address>,
    bcc: Vec<Address>,
    subject: String,
    text_body: Option<String>,
    html_body: Option<String>,
}

impl Message {
    /// Start a fluent builder.
    #[must_use]
    pub fn builder() -> MessageBuilder {
        MessageBuilder::default()
    }

    /// Envelope / header `From` address.
    #[must_use]
    pub fn from(&self) -> &Address {
        &self.from
    }

    /// Primary recipients.
    #[must_use]
    pub fn to(&self) -> &[Address] {
        &self.to
    }

    /// Carbon-copy recipients.
    #[must_use]
    pub fn cc(&self) -> &[Address] {
        &self.cc
    }

    /// Blind carbon-copy recipients.
    #[must_use]
    pub fn bcc(&self) -> &[Address] {
        &self.bcc
    }

    /// Subject header value (validated free of CR/LF).
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Plain-text body, if set.
    #[must_use]
    pub fn text_body(&self) -> Option<&str> {
        self.text_body.as_deref()
    }

    /// HTML body, if set.
    #[must_use]
    pub fn html_body(&self) -> Option<&str> {
        self.html_body.as_deref()
    }
}

/// Fluent builder for [`Message`].
///
/// Address and subject validation errors are deferred until [`MessageBuilder::build`].
#[derive(Clone, Debug, Default)]
pub struct MessageBuilder {
    from: Option<Address>,
    to: Vec<Address>,
    cc: Vec<Address>,
    bcc: Vec<Address>,
    subject: Option<String>,
    text_body: Option<String>,
    html_body: Option<String>,
    error: Option<MailError>,
}

impl MessageBuilder {
    /// Set the `From` address.
    #[must_use]
    pub fn from(mut self, address: impl AsRef<str>) -> Self {
        self.set_address("from", address.as_ref(), |this, parsed| {
            this.from = Some(parsed);
        });
        self
    }

    /// Append a `To` recipient.
    #[must_use]
    pub fn to(mut self, address: impl AsRef<str>) -> Self {
        self.set_address("to", address.as_ref(), |this, parsed| {
            this.to.push(parsed);
        });
        self
    }

    /// Append a `Cc` recipient.
    #[must_use]
    pub fn cc(mut self, address: impl AsRef<str>) -> Self {
        self.set_address("cc", address.as_ref(), |this, parsed| {
            this.cc.push(parsed);
        });
        self
    }

    /// Append a `Bcc` recipient.
    #[must_use]
    pub fn bcc(mut self, address: impl AsRef<str>) -> Self {
        self.set_address("bcc", address.as_ref(), |this, parsed| {
            this.bcc.push(parsed);
        });
        self
    }

    /// Set the subject (must not contain CR/LF or other control characters).
    #[must_use]
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        if self.error.is_some() {
            return self;
        }
        let subject = subject.into();
        if let Err(error) = reject_header_injection("subject", &subject) {
            self.error = Some(error);
            return self;
        }
        self.subject = Some(subject);
        self
    }

    /// Set the plain-text body (may contain newlines).
    #[must_use]
    pub fn text_body(mut self, body: impl Into<String>) -> Self {
        self.text_body = Some(body.into());
        self
    }

    /// Set the HTML body (may contain newlines).
    #[must_use]
    pub fn html_body(mut self, body: impl Into<String>) -> Self {
        self.html_body = Some(body.into());
        self
    }

    /// Finish the message after validating required fields and headers.
    ///
    /// # Errors
    ///
    /// Returns a deferred address/subject error, [`MailError::MissingFrom`],
    /// or [`MailError::NoRecipients`].
    pub fn build(self) -> Result<Message, MailError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let from = self.from.ok_or(MailError::MissingFrom)?;
        if self.to.is_empty() && self.cc.is_empty() && self.bcc.is_empty() {
            return Err(MailError::NoRecipients);
        }
        Ok(Message {
            from,
            to: self.to,
            cc: self.cc,
            bcc: self.bcc,
            subject: self.subject.unwrap_or_default(),
            text_body: self.text_body,
            html_body: self.html_body,
        })
    }

    fn set_address(
        &mut self,
        field: &'static str,
        raw: &str,
        assign: impl FnOnce(&mut Self, Address),
    ) {
        if self.error.is_some() {
            return;
        }
        // Treat CR/LF (and other controls) in address header fields as injection.
        if contains_control_char(raw) {
            self.error = Some(MailError::HeaderInjection { field });
            return;
        }
        match Address::parse(raw) {
            Ok(parsed) => assign(self, parsed),
            Err(error) => self.error = Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_multipart_bodies() {
        let message = Message::builder()
            .from("noreply@example.com")
            .to("user@example.com")
            .subject("Hello")
            .text_body("plain")
            .html_body("<p>html</p>")
            .build()
            .expect("valid message");
        assert_eq!(message.text_body(), Some("plain"));
        assert_eq!(message.html_body(), Some("<p>html</p>"));
    }

    #[test]
    fn rejects_crlf_in_subject() {
        let error = Message::builder()
            .from("noreply@example.com")
            .to("user@example.com")
            .subject("Hello\r\nBcc: evil@example.com")
            .build()
            .expect_err("header injection");
        assert_eq!(error, MailError::HeaderInjection { field: "subject" });
    }

    #[test]
    fn rejects_empty_recipients() {
        let error = Message::builder()
            .from("noreply@example.com")
            .subject("Hello")
            .build()
            .expect_err("no recipients");
        assert_eq!(error, MailError::NoRecipients);
    }
}
