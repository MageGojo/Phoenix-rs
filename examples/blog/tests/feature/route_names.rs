use phoenix::prelude::{Request, RouteBuildError, Routes, UrlGenerationError};

#[test]
fn named_routes_generate_urls_with_parameters_and_group_prefixes() {
    let router = test_routes().build().expect("routes should build");

    assert_eq!(
        router
            .url("users.show", &[("user", "Ada Lovelace_~.profile")])
            .expect("named route should generate"),
        "/users/Ada%20Lovelace_~.profile"
    );
    assert_eq!(
        router
            .url("admin.dashboard", &[])
            .expect("group name should be prefixed"),
        "/admin/dashboard"
    );
}

#[test]
fn named_routes_report_unknown_names_and_missing_parameters() {
    let router = test_routes().build().expect("routes should build");

    assert!(matches!(
        router.url("users.show", &[]),
        Err(UrlGenerationError::MissingParameter { parameter, .. }) if parameter == "user"
    ));
    assert_eq!(
        router.url("missing", &[]),
        Err(UrlGenerationError::UnknownRoute("missing".to_owned()))
    );
}

fn test_routes() -> Routes {
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(async {
            let store = phoenix_blog_example::models::AuthStore::in_memory()
                .await
                .expect("in-memory auth store should build");
            store
                .seed_demo_users()
                .await
                .expect("demo users should seed");
            store
        });
    phoenix_blog_example::web_routes_with_store(&store)
}

#[test]
fn duplicate_route_names_fail_during_build() {
    async fn handler(_request: Request) -> &'static str {
        "ok"
    }

    let error = Routes::new()
        .get("/first", handler)
        .name("duplicate")
        .get("/second", handler)
        .name("duplicate")
        .build()
        .expect_err("duplicate names must fail");

    assert!(matches!(
        error,
        RouteBuildError::DuplicateName(name) if name == "duplicate"
    ));
}
