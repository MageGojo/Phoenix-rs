use phoenix::prelude::{Method, Request, StatusCode};

#[tokio::test]
async fn website_frontend_and_admin_share_one_project_but_keep_routes_isolated() {
    let application = phoenix_multi_app_example::application().unwrap();

    for (path, expected) in [
        ("/", "Official website [website]"),
        ("/app", "Customer frontend [frontend]"),
        ("/app/account", "Customer frontend [frontend]"),
        ("/admin", "Administration [admin]"),
        ("/admin/users", "Administration [admin]"),
    ] {
        let response = application
            .handle(Request::new(Method::GET, path.parse().unwrap()))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.body(), expected, "{path}");
    }

    let missing = application
        .handle(Request::new(Method::GET, "/administrator".parse().unwrap()))
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    assert_eq!(application.router().url("website.home", &[]).unwrap(), "/");
    assert_eq!(
        application.router().url("frontend.account", &[]).unwrap(),
        "/app/account"
    );
    assert_eq!(
        application.router().url("admin.users.index", &[]).unwrap(),
        "/admin/users"
    );
}
