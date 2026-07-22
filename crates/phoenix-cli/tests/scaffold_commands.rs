use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn temporary_directory() -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("valid clock")
        .as_nanos();
    env::temp_dir().join(format!("phoenix-cli-command-{}-{id}", std::process::id()))
}

fn run(cwd: &Path, arguments: &[&str]) {
    let status = Command::new(env!("CARGO_BIN_EXE_px"))
        .args(arguments)
        .current_dir(cwd)
        .status()
        .expect("CLI should start");
    assert!(status.success(), "command failed: {arguments:?}");
}

#[test]
fn command_surface_generates_and_registers_every_artifact() {
    let root = temporary_directory();
    fs::create_dir_all(&root).expect("temporary root");
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("framework root");
    run(
        &root,
        &[
            "new",
            "demo",
            "--framework-path",
            framework.to_str().expect("UTF-8 path"),
            "--no-install",
            "--no-git",
        ],
    );
    let project = root.join("demo");

    run(&project, &["make:controller", "Report", "--route"]);
    run(&project, &["make:model", "Post", "--migration"]);
    run(&project, &["make:migration", "add_status_to_posts"]);
    run(&project, &["make:request", "PublishPost"]);
    run(&project, &["make:resource", "PostSummary"]);
    run(&project, &["make:middleware", "RequireLogin"]);
    run(&project, &["make:page", "reports/index"]);
    run(&project, &["make:island", "Counter"]);

    for path in [
        "app/controllers/report_controller.rs",
        "app/models/post.rs",
        "app/requests/publish_post_request.rs",
        "app/resources/post_summary_resource.rs",
        "app/middleware/require_login_middleware.rs",
        "routes/reports.rs",
        "views/pages/reports/index.tsx",
        "views/islands/counter.tsx",
    ] {
        assert!(project.join(path).is_file(), "missing {path}");
    }
    let migrations =
        fs::read_to_string(project.join("database/migrations/mod.rs")).expect("migration registry");
    assert_eq!(migrations.matches("phoenix:migration:").count(), 2);
    fs::remove_dir_all(root).expect("remove temporary project");
}
