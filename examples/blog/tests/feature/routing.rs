use std::future::{Ready, ready};

use phoenix::prelude::{Method, Request, Routes, StatusCode, Uri};

#[tokio::test]
async fn route_parameters_and_http_methods_are_dispatched() {
    let application = phoenix_blog_example::application().expect("routes should build");

    let response = application
        .handle(Request::new(
            Method::GET,
            Uri::from_static("/users/Ada%20Lovelace"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(response.body()).expect("controller should return JSON");
    assert_eq!(json["user"], "Ada Lovelace");
    assert_eq!(json["route"], "users.show");

    let response = application
        .handle(Request::new(Method::POST, Uri::from_static("/users/42")))
        .await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response
            .headers()
            .get("allow")
            .and_then(|value| value.to_str().ok()),
        Some("GET, HEAD")
    );

    let response = application
        .handle(Request::new(Method::GET, Uri::from_static("/missing")))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn common_http_methods_head_and_options_are_supported() {
    fn handler(_request: Request) -> Ready<&'static str> {
        ready("ok")
    }

    let router = Routes::new()
        .get("/head", handler)
        .put("/resource", handler)
        .patch("/resource", handler)
        .delete("/resource", handler)
        .build()
        .expect("routes should build");

    for method in [Method::PUT, Method::PATCH, Method::DELETE] {
        let response = router
            .handle(Request::new(method, Uri::from_static("/resource")))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), "ok");
    }

    let response = router
        .handle(Request::new(Method::HEAD, Uri::from_static("/head")))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.body().is_empty());

    let response = router
        .handle(Request::new(Method::OPTIONS, Uri::from_static("/resource")))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get("allow")
            .and_then(|value| value.to_str().ok()),
        Some("DELETE, PATCH, PUT")
    );
}
