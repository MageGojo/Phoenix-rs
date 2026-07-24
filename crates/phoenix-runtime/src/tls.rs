//! Direct rustls TLS listener support.

use std::{io::Cursor, path::Path, sync::Arc, time::Duration};

use rustls::ServerConfig;
use thiserror::Error;

use crate::{ALPN_HTTP_1_1, ALPN_HTTP_2, DEFAULT_TLS_HANDSHAKE_TIMEOUT, HttpProtocol};

/// Rustls server configuration with Phoenix ALPN and handshake policy.
#[derive(Clone)]
pub struct TlsConfig {
    pub(crate) server_config: Arc<ServerConfig>,
    pub(crate) handshake_timeout: Duration,
}

impl TlsConfig {
    /// Load a certificate chain and private key from PEM bytes.
    ///
    /// # Errors
    ///
    /// Returns an error for unreadable PEM, missing material, or an invalid key/certificate pair.
    pub fn from_pem(
        certificate_pem: &[u8],
        private_key_pem: &[u8],
    ) -> Result<Self, TlsConfigError> {
        let certificates = rustls_pemfile::certs(&mut Cursor::new(certificate_pem))
            .collect::<Result<Vec<_>, _>>()?;
        if certificates.is_empty() {
            return Err(TlsConfigError::MissingCertificate);
        }
        let private_key = rustls_pemfile::private_key(&mut Cursor::new(private_key_pem))?
            .ok_or(TlsConfigError::MissingPrivateKey)?;
        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certificates, private_key)?;
        Ok(Self::from_server_config(server_config))
    }

    /// Load PEM material from local files during application startup.
    ///
    /// # Errors
    ///
    /// Returns an error when either file cannot be read or parsed.
    pub fn from_files(
        certificate_path: impl AsRef<Path>,
        private_key_path: impl AsRef<Path>,
    ) -> Result<Self, TlsConfigError> {
        let certificate_pem = std::fs::read(certificate_path)?;
        let private_key_pem = std::fs::read(private_key_path)?;
        Self::from_pem(&certificate_pem, &private_key_pem)
    }

    /// Wrap an advanced rustls configuration. Phoenix supplies HTTP ALPN defaults when absent.
    #[must_use]
    pub fn from_server_config(mut server_config: ServerConfig) -> Self {
        if server_config.alpn_protocols.is_empty() {
            server_config.alpn_protocols = vec![ALPN_HTTP_2.to_vec(), ALPN_HTTP_1_1.to_vec()];
        }
        Self {
            server_config: Arc::new(server_config),
            handshake_timeout: DEFAULT_TLS_HANDSHAKE_TIMEOUT,
        }
    }

    /// Set a hard deadline for completing the TLS handshake.
    ///
    /// # Errors
    ///
    /// Returns an error when the timeout is zero.
    pub fn handshake_timeout(mut self, timeout: Duration) -> Result<Self, TlsConfigError> {
        if timeout.is_zero() {
            return Err(TlsConfigError::InvalidHandshakeTimeout);
        }
        self.handshake_timeout = timeout;
        Ok(self)
    }

    #[must_use]
    pub fn alpn_protocols(&self) -> &[Vec<u8>] {
        &self.server_config.alpn_protocols
    }

    pub(crate) fn for_http_protocol(&self, protocol: HttpProtocol) -> Self {
        let mut server_config = (*self.server_config).clone();
        server_config.alpn_protocols = match protocol {
            HttpProtocol::Auto => vec![ALPN_HTTP_2.to_vec(), ALPN_HTTP_1_1.to_vec()],
            HttpProtocol::Http1Only => vec![ALPN_HTTP_1_1.to_vec()],
            HttpProtocol::Http2Only => vec![ALPN_HTTP_2.to_vec()],
        };
        Self {
            server_config: Arc::new(server_config),
            handshake_timeout: self.handshake_timeout,
        }
    }
}

impl std::fmt::Debug for TlsConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TlsConfig")
            .field("alpn_protocols", &self.server_config.alpn_protocols)
            .field("handshake_timeout", &self.handshake_timeout)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub enum TlsConfigError {
    #[error("TLS PEM I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS certificate PEM does not contain a certificate")]
    MissingCertificate,
    #[error("TLS private-key PEM does not contain a supported private key")]
    MissingPrivateKey,
    #[error("TLS certificate or private key is invalid: {0}")]
    Rustls(#[from] rustls::Error),
    #[error("TLS handshake timeout must be greater than zero")]
    InvalidHandshakeTimeout,
}

