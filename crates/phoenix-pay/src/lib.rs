//! Unified payment abstraction for Phoenix.
//!
//! Money is always an integer count of minor units ([`Amount`], 分 for CNY);
//! providers implement [`PaymentProvider`]; the [`PayManager`] persists orders
//! through a [`PaymentStore`] and processes asynchronous notifications
//! idempotently on `(provider, out_trade_no)`. [`PayFeature`] installs the
//! webhook routes and the `payments` migration via `phoenix-plugin`.
//!
//! [`MockProvider`] is fully functional for tests; the real `WeChat` Native
//! (`APIv3`) and Alipay F2F (RSA2) gateways live in [`WechatNativeProvider`]
//! and [`AlipayF2FProvider`], talking HTTP through the pluggable [`PayHttp`]
//! transport. See `docs/PAYMENTS.md`.

#![forbid(unsafe_code)]

mod alipay;
mod amount;
mod config;
mod crypto;
mod db_store;
mod error;
mod feature;
mod gateway;
mod manager;
mod order;
mod provider;
mod reconcile;
mod refund;
mod status;
mod store;
mod transport;
mod wechat;

pub use amount::{Amount, Currency};
pub use config::{AlipayF2FConfig, Secret, WechatNativeConfig};
pub use db_store::{DbPaymentStore, PaymentRow, RefundRow};
pub use error::PayError;
pub use feature::{
    PAYMENTS_MIGRATION_ID, PayFeature, REFUNDS_MIGRATION_ID, payments_migration, refunds_migration,
};
pub use gateway::{AlipayF2FProvider, WechatNativeProvider};
pub use manager::{NotifyOutcome, PayManager, PayManagerBuilder};
pub use order::{
    CreateOrder, NotifyEvent, NotifyRequest, PaymentAction, PaymentIntent, PaymentRecord,
};
pub use provider::{MockProvider, PaymentProvider};
pub use reconcile::{Bill, BillEntry, Discrepancy, Reconciliation, parse_bill_csv, reconcile};
pub use refund::{RefundOrder, RefundReceipt, RefundRecord, RefundStatus};
pub use status::PaymentStatus;
pub use store::{MemoryPaymentStore, PaymentStore};
pub use transport::{GatewayRequest, GatewayResponse, HyperPayHttp, PayHttp};

/// Convenience re-exports for application code.
pub mod prelude {
    pub use crate::{
        AlipayF2FConfig, AlipayF2FProvider, Amount, Bill, BillEntry, CreateOrder, Currency,
        DbPaymentStore, Discrepancy, HyperPayHttp, MemoryPaymentStore, MockProvider, NotifyEvent,
        NotifyOutcome, NotifyRequest, PayError, PayFeature, PayHttp, PayManager, PaymentAction,
        PaymentIntent, PaymentProvider, PaymentRecord, PaymentRow, PaymentStatus, PaymentStore,
        Reconciliation, RefundOrder, RefundReceipt, RefundRecord, RefundRow, RefundStatus, Secret,
        WechatNativeConfig, WechatNativeProvider, reconcile,
    };
}
