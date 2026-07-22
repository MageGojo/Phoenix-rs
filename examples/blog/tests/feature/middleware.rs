use phoenix::{
    http::HeaderValue,
    prelude::{Method, Next, Request, Routes, StatusCode, Uri, middleware_fn},
};

#[tokio::test]
async fn global_and_group_middleware_wrap_controller_responses() {
    let application = phoenix_blog_example::application().expect("routes should build");

    let response = application
        .handle(Request::new(
            Method::GET,
            Uri::from_static("/admin/dashboard"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get("x-powered-by"),
        Some(&HeaderValue::from_static("Phoenix"))
    );

    let mut request = Request::new(Method::GET, Uri::from_static("/admin/dashboard"));
    request
        .headers_mut()
        .insert("x-example-token", HeaderValue::from_static("secret"));
    let response = application.handle(request).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.body(), "admin dashboard");
    assert_eq!(
        response.headers().get("x-powered-by"),
        Some(&HeaderValue::from_static("Phoenix"))
    );
}

#[tokio::test]
async fn middleware_can_be_attached_to_one_route() {
    let router = Routes::new()
        .get("/guarded", |_request| std::future::ready("guarded"))
        .middleware(middleware_fn(|request: Request, next: Next| async move {
            let mut response = next.run(request).await;
            response
                .headers_mut()
                .insert("x-route-middleware", HeaderValue::from_static("applied"));
            response
        }))
        .get("/plain", |_request| std::future::ready("plain"))
        .build()
        .expect("routes should build");

    let guarded = router
        .handle(Request::new(Method::GET, Uri::from_static("/guarded")))
        .await;
    assert_eq!(
        guarded.headers().get("x-route-middleware"),
        Some(&HeaderValue::from_static("applied"))
    );

    let plain = router
        .handle(Request::new(Method::GET, Uri::from_static("/plain")))
        .await;
    assert!(plain.headers().get("x-route-middleware").is_none());
}
