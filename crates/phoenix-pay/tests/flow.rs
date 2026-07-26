//! End-to-end tests: mock provider flow, notify idempotency, and the
//! `PayFeature` webhook routes installed through `FeatureSet`.

use std::sync::Arc;

use phoenix_http::{Method, Request, StatusCode, Uri};
use phoenix_pay::prelude::*;
use phoenix_pay::{PAYMENTS_MIGRATION_ID, REFUNDS_MIGRATION_ID};
use phoenix_plugin::FeatureSet;

fn mock_manager() -> (Arc<PayManager>, MockProvider) {
    let provider = MockProvider::new();
    let manager = PayManager::builder()
        .provider(Arc::new(provider.clone()))
        .build();
    (Arc::new(manager), provider)
}

fn order(out_trade_no: &str) -> CreateOrder {
    CreateOrder::new(out_trade_no, Amount::cny(1234), "会员月卡")
}

#[tokio::test]
async fn mock_full_flow_create_notify_paid() {
    let (manager, provider) = mock_manager();

    let intent = manager.create("mock", order("T100")).await.expect("create");
    assert_eq!(intent.provider, "mock");
    assert_eq!(intent.amount, Amount::cny(1234));
    let PaymentAction::QrCode(text) = &intent.action else {
        panic!("mock create must return a QR code, got {:?}", intent.action);
    };
    assert!(text.contains("T100"));

    let stored = manager.find_order("mock", "T100").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Pending);
    assert_eq!(
        manager.query("mock", "T100").await,
        Ok(PaymentStatus::Pending)
    );

    // Payer scans and pays; the platform posts the notification.
    let body = provider.mark_paid("T100").expect("mark paid");
    let outcome = manager
        .handle_notify("mock", NotifyRequest::from_body(body.clone()))
        .await
        .expect("first notify");
    let NotifyOutcome::Processed(event) = outcome else {
        panic!("first notify must be Processed, got {outcome:?}");
    };
    assert_eq!(event.out_trade_no, "T100");
    assert_eq!(event.status, PaymentStatus::Paid);

    let stored = manager.find_order("mock", "T100").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Paid);
    assert_eq!(stored.notify_payload.as_deref(), Some(body.as_str()));
    assert_eq!(manager.query("mock", "T100").await, Ok(PaymentStatus::Paid));
}

#[tokio::test]
async fn duplicate_notify_is_idempotent() {
    let (manager, provider) = mock_manager();
    manager.create("mock", order("T200")).await.expect("create");
    let body = provider.mark_paid("T200").expect("mark paid");

    let first = manager
        .handle_notify("mock", NotifyRequest::from_body(body.clone()))
        .await
        .expect("first notify");
    assert!(matches!(first, NotifyOutcome::Processed(_)));

    for _ in 0..3 {
        let replay = manager
            .handle_notify("mock", NotifyRequest::from_body(body.clone()))
            .await
            .expect("replayed notify");
        assert!(
            matches!(replay, NotifyOutcome::AlreadyProcessed(_)),
            "replays must be acknowledged without a second transition"
        );
    }
    let stored = manager.find_order("mock", "T200").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Paid);
}

#[tokio::test]
async fn create_rejects_duplicates_and_invalid_orders() {
    let (manager, _provider) = mock_manager();
    manager.create("mock", order("T300")).await.expect("create");
    assert!(matches!(
        manager.create("mock", order("T300")).await,
        Err(PayError::DuplicateOrder { .. })
    ));
    assert_eq!(
        manager
            .create("mock", CreateOrder::new("T301", Amount::cny(0), "x"))
            .await,
        Err(PayError::InvalidOrder("amount must be greater than zero"))
    );
    assert!(matches!(
        manager.create("nope", order("T302")).await,
        Err(PayError::UnknownProvider(_))
    ));
}

#[tokio::test]
async fn notify_for_unknown_order_fails() {
    let (manager, _provider) = mock_manager();
    let body = MockProvider::paid_notify_body("GHOST");
    assert!(matches!(
        manager
            .handle_notify("mock", NotifyRequest::from_body(body))
            .await,
        Err(PayError::OrderNotFound { .. })
    ));
}

fn feature_router(manager: &Arc<PayManager>) -> phoenix_routing::Router {
    FeatureSet::new()
        .plugin(PayFeature::new(Arc::clone(manager)))
        .expect("install pay feature")
        .into_parts()
        .routes
        .build()
        .expect("build router")
}

fn post(path: &str, body: &str) -> Request {
    Request::from_parts(
        Method::POST,
        path.parse::<Uri>().expect("uri"),
        phoenix_http::HeaderMap::new(),
        body.to_owned().into_bytes().into(),
    )
}

#[tokio::test]
async fn feature_installs_named_routes_and_migration() {
    let (manager, _provider) = mock_manager();
    let parts = FeatureSet::new()
        .plugin(PayFeature::new(manager))
        .expect("install pay feature")
        .into_parts();

    assert_eq!(
        parts
            .migrations
            .iter()
            .map(phoenix_database::Migration::id)
            .collect::<Vec<_>>(),
        vec![PAYMENTS_MIGRATION_ID, REFUNDS_MIGRATION_ID]
    );

    let router = parts.routes.build().expect("router");
    assert_eq!(
        router.url("pay.notify.wechat", &[]).unwrap(),
        "/pay/notify/wechat"
    );
    assert_eq!(
        router.url("pay.notify.alipay", &[]).unwrap(),
        "/pay/notify/alipay"
    );
    assert_eq!(
        router.url("pay.notify.mock", &[]).unwrap(),
        "/pay/notify/mock"
    );
    assert_eq!(
        router
            .url(
                "pay.orders.show",
                &[("provider", "mock"), ("out_trade_no", "T1")]
            )
            .unwrap(),
        "/pay/orders/mock/T1"
    );
}

#[tokio::test]
async fn webhook_route_processes_and_deduplicates_notify() {
    let (manager, provider) = mock_manager();
    let router = feature_router(&manager);

    manager.create("mock", order("T400")).await.expect("create");
    let body = provider.mark_paid("T400").expect("mark paid");

    let response = router.handle(post("/pay/notify/mock", &body)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    assert_eq!(payload["code"], "SUCCESS");
    assert_eq!(payload["duplicate"], false);

    let response = router.handle(post("/pay/notify/mock", &body)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    assert_eq!(payload["duplicate"], true);

    let stored = manager.find_order("mock", "T400").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Paid);
}

#[tokio::test]
async fn wechat_webhook_rejects_unsigned_notify() {
    let manager = Arc::new(
        PayManager::builder()
            .provider(Arc::new(WechatNativeProvider::new(
                toml::from_str(
                    r#"
                    app_id = "wx1"
                    mch_id = "m1"
                    mch_serial_no = "s1"
                    api_v3_key = "k"
                    private_key_path = "key.pem"
                    notify_url = "https://example.com/pay/notify/wechat"
                    "#,
                )
                .expect("config"),
            )))
            .build(),
    );
    let router = feature_router(&manager);
    // No Wechatpay-* signature headers: the webhook must refuse the payload
    // outright (400), never produce a NotifyEvent from unverified bytes.
    let response = router.handle(post("/pay/notify/wechat", "{}")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn manager_close_moves_order_to_closed() {
    let (manager, _provider) = mock_manager();
    manager.create("mock", order("T600")).await.expect("create");
    manager.close("mock", "T600").await.expect("close");
    let stored = manager.find_order("mock", "T600").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Closed);
    assert_eq!(
        manager.query("mock", "T600").await,
        Ok(PaymentStatus::Closed)
    );
    // Closing a closed order is an illegal transition, not a silent no-op.
    assert!(matches!(
        manager.close("mock", "T600").await,
        Err(PayError::InvalidTransition { .. })
    ));
}

#[tokio::test]
async fn order_show_route_returns_stored_status() {
    let (manager, _provider) = mock_manager();
    let router = feature_router(&manager);
    manager.create("mock", order("T500")).await.expect("create");

    let request = Request::from_parts(
        Method::GET,
        "/pay/orders/mock/T500".parse::<Uri>().unwrap(),
        phoenix_http::HeaderMap::new(),
        phoenix_http::Bytes::new(),
    );
    let response = router.handle(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    assert_eq!(payload["status"], "pending");
    assert_eq!(payload["amount"]["minor"], 1234);

    let request = Request::from_parts(
        Method::GET,
        "/pay/orders/mock/NOPE".parse::<Uri>().unwrap(),
        phoenix_http::HeaderMap::new(),
        phoenix_http::Bytes::new(),
    );
    let response = router.handle(request).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
