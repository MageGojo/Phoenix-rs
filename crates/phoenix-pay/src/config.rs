use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

/// Secret configuration value: zeroized on drop, redacted in `Debug`.
///
/// Mirrors `phoenix_config::SecretValue`, but is deserializable so payment
/// channel configs can live in `config/*.toml` (values injected from `.env`).
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// Wrap a secret string.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Read the secret. Never log or `Debug`-print the result.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::new)
    }
}

fn default_alipay_gateway() -> String {
    "https://openapi.alipay.com/gateway.do".to_owned()
}

fn default_sign_type() -> String {
    "RSA2".to_owned()
}

/// `WeChat` Pay Native (扫码支付, API v3) channel configuration, consumed by
/// [`crate::WechatNativeProvider`]. See `docs/PAYMENTS.md`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WechatNativeConfig {
    /// 公众号 / 开放平台 appid bound to the merchant.
    pub app_id: String,
    /// 商户号 (`mch_id`).
    pub mch_id: String,
    /// 商户 API 证书序列号 (`serial_no`) used in the `Authorization` header.
    pub mch_serial_no: String,
    /// `APIv3` 密钥, used to decrypt notification resources (AES-256-GCM).
    pub api_v3_key: Secret,
    /// Path to the merchant RSA private key PEM (`apiclient_key.pem`).
    pub private_key_path: PathBuf,
    /// Path to a cached `WeChat` platform certificate PEM, if pre-downloaded.
    #[serde(default)]
    pub platform_cert_path: Option<PathBuf>,
    /// Absolute HTTPS URL `WeChat` calls back with payment notifications.
    pub notify_url: String,
}

/// Alipay 当面付 (Face-to-Face, `alipay.trade.precreate`) channel
/// configuration, consumed by [`crate::AlipayF2FProvider`]. See
/// `docs/PAYMENTS.md`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlipayF2FConfig {
    /// 开放平台应用 appid.
    pub app_id: String,
    /// `OpenAPI` gateway, override for sandbox.
    #[serde(default = "default_alipay_gateway")]
    pub gateway_url: String,
    /// 应用 RSA2 私钥 (PEM body) used to sign requests.
    pub app_private_key: Secret,
    /// 支付宝公钥 (PEM body) used to verify responses and notifications.
    pub alipay_public_key: Secret,
    /// Signature algorithm; only `RSA2` is planned.
    #[serde(default = "default_sign_type")]
    pub sign_type: String,
    /// Absolute HTTPS URL Alipay calls back with payment notifications.
    pub notify_url: String,
    /// Certificate-mode paths (公钥证书模式), reserved: certificate mode is
    /// not wired up yet (see `docs/PAYMENTS.md` follow-ups).
    #[serde(default)]
    pub app_cert_path: Option<PathBuf>,
    /// Alipay root certificate path for certificate mode.
    #[serde(default)]
    pub alipay_root_cert_path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wechat_config_deserializes_and_redacts() {
        let config: WechatNativeConfig = toml::from_str(
            r#"
            app_id = "wx1234567890"
            mch_id = "1900000001"
            mch_serial_no = "5157F09EFDC096DE15EBE81A47057A72"
            api_v3_key = "0123456789abcdef0123456789abcdef"
            private_key_path = "storage/certs/apiclient_key.pem"
            notify_url = "https://shop.example.com/pay/notify/wechat"
            "#,
        )
        .expect("wechat config");
        assert_eq!(config.app_id, "wx1234567890");
        assert_eq!(
            config.api_v3_key.expose(),
            "0123456789abcdef0123456789abcdef"
        );
        assert!(config.platform_cert_path.is_none());

        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("0123456789abcdef"));
    }

    #[test]
    fn alipay_config_defaults_and_redacts() {
        let config: AlipayF2FConfig = toml::from_str(
            r#"
            app_id = "2021000000000000"
            app_private_key = "MIIEvQIBADANBg-secret-key"
            alipay_public_key = "MIIBIjANBg-public-key"
            notify_url = "https://shop.example.com/pay/notify/alipay"
            "#,
        )
        .expect("alipay config");
        assert_eq!(config.gateway_url, "https://openapi.alipay.com/gateway.do");
        assert_eq!(config.sign_type, "RSA2");
        assert_eq!(config.app_private_key.expose(), "MIIEvQIBADANBg-secret-key");

        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("MIIEvQIBADANBg-secret-key"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let error = toml::from_str::<AlipayF2FConfig>(
            r#"
            app_id = "x"
            app_private_key = "k"
            alipay_public_key = "p"
            notify_url = "https://example.com/n"
            typo_field = true
            "#,
        )
        .expect_err("unknown field");
        assert!(error.to_string().contains("typo_field"));
    }
}
