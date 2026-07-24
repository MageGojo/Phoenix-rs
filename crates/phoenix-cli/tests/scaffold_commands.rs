use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_TEMPORARY_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn temporary_directory() -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("valid clock")
        .as_nanos();
    let sequence = NEXT_TEMPORARY_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!(
        "phoenix-cli-command-{}-{id}-{sequence}",
        std::process::id()
    ))
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
            "--database",
            "sqlite",
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
    run(&project, &["make:command", "Update"]);

    for path in [
        "app/controllers/report_controller.rs",
        "app/models/post.rs",
        "app/requests/publish_post_request.rs",
        "app/resources/post_summary_resource.rs",
        "app/middleware/require_login_middleware.rs",
        "app/commands/update.rs",
        "routes/reports.rs",
        "views/pages/reports/index.tsx",
        "views/islands/counter.tsx",
    ] {
        assert!(project.join(path).is_file(), "missing {path}");
    }
    let commands = fs::read_to_string(project.join("app/commands/mod.rs")).expect("commands");
    assert!(commands.contains("pub mod update;"));
    assert!(commands.contains("update,"));
    let main = fs::read_to_string(project.join("src/main.rs")).expect("main");
    assert!(main.contains("Console::new"));
    assert!(main.contains("commands::registry()"));
    let migrations =
        fs::read_to_string(project.join("database/migrations/mod.rs")).expect("migration registry");
    assert_eq!(migrations.matches("phoenix:migration:").count(), 2);
    fs::remove_dir_all(root).expect("remove temporary project");
}

#[cfg(unix)]
#[test]
fn database_commands_dispatch_to_the_project_management_binary() {
    use std::os::unix::fs::PermissionsExt;

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
            "--database",
            "sqlite",
            "--no-install",
            "--no-git",
        ],
    );
    let project = root.join("demo");
    let capture = root.join("cargo-invocations.txt");
    let fake_cargo = root.join("cargo");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\nprintf '%s' \"$PWD\" >> \"$PX_CAPTURE\"\nfor argument in \"$@\"; do printf '\\t%s' \"$argument\" >> \"$PX_CAPTURE\"; done\nprintf '\\n' >> \"$PX_CAPTURE\"\n",
    )
    .expect("fake cargo");
    let mut permissions = fs::metadata(&fake_cargo)
        .expect("fake cargo metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_cargo, permissions).expect("make fake cargo executable");

    for arguments in [
        vec!["status"],
        vec!["migrate"],
        vec!["rollback", "--step", "3"],
        vec!["fresh", "--seed"],
        vec!["seed"],
    ] {
        let status = Command::new(env!("CARGO_BIN_EXE_px"))
            .args(&arguments)
            .current_dir(&project)
            .env("CARGO", &fake_cargo)
            .env("PX_CAPTURE", &capture)
            .status()
            .expect("CLI should start");
        assert!(status.success(), "command failed: {arguments:?}");
    }

    let project = project.canonicalize().expect("canonical project path");
    let invocations = fs::read_to_string(capture).expect("captured cargo invocations");
    let expected = ["status", "migrate", "rollback\t3", "fresh\t--seed", "seed"];
    for (line, command) in invocations.lines().zip(expected) {
        assert_eq!(
            line,
            format!(
                "{}\trun\t--quiet\t--bin\tphoenix-manage\t--\t{command}",
                project.display()
            )
        );
    }
    assert_eq!(invocations.lines().count(), expected.len());
    fs::remove_dir_all(root).expect("remove temporary project");
}
