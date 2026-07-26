use std::sync::Arc;

use phoenix_database::Migration;
use phoenix_http::{Handler, IntoResponse, Json, Request, Response, StatusCode};
use phoenix_plugin::{Capability, Plugin};
use phoenix_routing::Routes;
use serde_json::json;

use crate::{NotifyOutcome, NotifyRequest, PayError, PayManager};

/// Ordered id of the `payments` table migration.
pub const PAYMENTS_MIGRATION_ID: &str = "202607260001";

/// Ordered id of the `payment_refunds` table migration.
pub const REFUNDS_MIGRATION_ID: &str = "202607260004";

/// The `payments` table: one row per `(provider, out_trade_no)` order.
///
/// `paid_at` is written the first time an order reaches `paid` and is what
/// daily reconciliation queries a window over, so it carries its own index.
///
/// SQL targets `SQLite` first (the workspace default); `PostgreSQL` accepts it,
/// `MySQL` needs an adjusted `DROP INDEX` — revisit with the DB-backed store.
#[must_use]
pub fn payments_migration() -> Migration {
    Migration::new(PAYMENTS_MIGRATION_ID, "create payments table")
        .up("CREATE TABLE IF NOT EXISTS payments (\
             id INTEGER PRIMARY KEY, \
             provider TEXT NOT NULL, \
             out_trade_no TEXT NOT NULL, \
             amount BIGINT NOT NULL, \
             currency TEXT NOT NULL, \
             status TEXT NOT NULL, \
             subject TEXT NOT NULL, \
             notify_payload TEXT, \
             paid_at TEXT, \
             created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, \
             updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")
        .up(
            "CREATE UNIQUE INDEX IF NOT EXISTS payments_provider_out_trade_no \
             ON payments (provider, out_trade_no)",
        )
        .up("CREATE INDEX IF NOT EXISTS payments_provider_paid_at ON payments (provider, paid_at)")
        .down("DROP INDEX IF EXISTS payments_provider_paid_at")
        .down("DROP INDEX IF EXISTS payments_provider_out_trade_no")
        .down("DROP TABLE IF EXISTS payments")
}

/// The `payment_refunds` table: one row per `(provider, out_refund_no)` refund.
///
/// Separate from `payments` because one paid order can carry several partial
/// refunds, each with its own number, amount, and outcome.
#[must_use]
pub fn refunds_migration() -> Migration {
    Migration::new(REFUNDS_MIGRATION_ID, "create payment_refunds table")
        .up("CREATE TABLE IF NOT EXISTS payment_refunds (\
             id INTEGER PRIMARY KEY, \
             provider TEXT NOT NULL, \
             out_trade_no TEXT NOT NULL, \
             out_refund_no TEXT NOT NULL, \
             refund_id TEXT, \
             amount BIGINT NOT NULL, \
             currency TEXT NOT NULL, \
             status TEXT NOT NULL, \
             reason TEXT, \
             created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")
        .up(
            "CREATE UNIQUE INDEX IF NOT EXISTS payment_refunds_provider_out_refund_no \
             ON payment_refunds (provider, out_refund_no)",
        )
        .up(
            "CREATE INDEX IF NOT EXISTS payment_refunds_provider_out_trade_no \
             ON payment_refunds (provider, out_trade_no)",
        )
        .down("DROP INDEX IF EXISTS payment_refunds_provider_out_trade_no")
        .down("DROP INDEX IF EXISTS payment_refunds_provider_out_refund_no")
        .down("DROP TABLE IF EXISTS payment_refunds")
}

/// Phoenix Feature installing payment webhook routes and the `payments`
/// migration.
///
/// Route names (namespaced by `FeatureSet`, plugin name `pay`):
///
/// | Name | Method + path | Purpose |
/// | --- | --- | --- |
/// | `pay.notify.wechat` | `POST /pay/notify/wechat` | `WeChat` asynchronous notify |
/// | `pay.notify.alipay` | `POST /pay/notify/alipay` | Alipay asynchronous notify |
/// | `pay.notify.mock` | `POST /pay/notify/mock` | Mock provider notify (dev/test) |
/// | `pay.orders.show` | `GET /pay/orders/{provider}/{out_trade_no}` | Stored order status |
///
/// Webhooks are called by the payment platform, not a browser: install these
/// routes WITHOUT the session/CSRF middleware stack (merge the `FeatureSet`
/// routes before applying `Csrf`, or keep them in their own group). Provider
/// notify authenticity is enforced by `PaymentProvider::verify_notify`, not by
/// CSRF tokens.
pub struct PayFeature {
    manager: Arc<PayManager>,
}

impl PayFeature {
    /// Wrap a configured [`PayManager`].
    #[must_use]
    pub fn new(manager: Arc<PayManager>) -> Self {
        Self { manager }
    }

    /// Shared handle to the manager (for application state / controllers).
    #[must_use]
    pub fn manager(&self) -> Arc<PayManager> {
        Arc::clone(&self.manager)
    }
}

impl std::fmt::Debug for PayFeature {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PayFeature")
            .field("manager", &self.manager)
            .finish()
    }
}

fn error_status(error: &PayError) -> StatusCode {
    match error {
        PayError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
        PayError::OrderNotFound { .. }
        | PayError::RefundNotFound { .. }
        | PayError::UnknownProvider(_) => StatusCode::NOT_FOUND,
        PayError::Store(_) | PayError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
        PayError::Gateway(_) | PayError::Reconcile(_) => StatusCode::BAD_GATEWAY,
        PayError::DuplicateOrder { .. } | PayError::DuplicateRefund { .. } => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    }
}

fn error_response(error: &PayError) -> Response {
    Json(json!({ "code": "FAIL", "message": error.to_string() }))
        .into_response()
        .with_status(error_status(error))
}

fn notify_handler(manager: Arc<PayManager>, provider_key: &'static str) -> impl Handler {
    move |request: Request| {
        let manager = Arc::clone(&manager);
        async move {
            let notify = NotifyRequest::new(request.headers().clone(), request.body().clone());
            match manager.handle_notify(provider_key, notify).await {
                Ok(NotifyOutcome::Processed(event)) => Json(json!({
                    "code": "SUCCESS",
                    "out_trade_no": event.out_trade_no,
                    "status": event.status,
                    "duplicate": false,
                }))
                .into_response(),
                Ok(NotifyOutcome::AlreadyProcessed(event)) => Json(json!({
                    "code": "SUCCESS",
                    "out_trade_no": event.out_trade_no,
                    "status": event.status,
                    "duplicate": true,
                }))
                .into_response(),
                Err(error) => error_response(&error),
            }
        }
    }
}

async fn show_order(manager: Arc<PayManager>, request: Request) -> Response {
    let (Some(provider), Some(out_trade_no)) =
        (request.param("provider"), request.param("out_trade_no"))
    else {
        return error_response(&PayError::InvalidOrder("missing route parameters"));
    };
    match manager.find_order(provider, out_trade_no).await {
        Ok(Some(record)) => Json(json!({
            "provider": record.provider,
            "out_trade_no": record.out_trade_no,
            "amount": record.amount,
            "subject": record.subject,
            "status": record.status,
        }))
        .into_response(),
        Ok(None) => error_response(&PayError::OrderNotFound {
            provider: provider.to_owned(),
            out_trade_no: out_trade_no.to_owned(),
        }),
        Err(error) => error_response(&error),
    }
}

impl Plugin for PayFeature {
    fn name(&self) -> &'static str {
        "pay"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Routes, Capability::Migrations]
    }

    fn routes(&self) -> Routes {
        let manager = Arc::clone(&self.manager);
        Routes::new()
            .post(
                "/pay/notify/wechat",
                notify_handler(Arc::clone(&manager), crate::WechatNativeProvider::KEY),
            )
            .name("notify.wechat")
            .post(
                "/pay/notify/alipay",
                notify_handler(Arc::clone(&manager), crate::AlipayF2FProvider::KEY),
            )
            .name("notify.alipay")
            .post(
                "/pay/notify/mock",
                notify_handler(Arc::clone(&manager), crate::MockProvider::KEY),
            )
            .name("notify.mock")
            .get("/pay/orders/{provider}/{out_trade_no}", move |request| {
                let manager = Arc::clone(&manager);
                async move { show_order(manager, request).await }
            })
            .name("orders.show")
    }

    fn migrations(&self) -> Vec<Migration> {
        vec![payments_migration(), refunds_migration()]
    }
}
