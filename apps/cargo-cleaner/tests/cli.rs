use serde_json::Value;
use std::{
    fs,
    os::unix::fs::symlink,
    path::Path,
    process::{Command, Output},
};

const TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

fn project(path: &Path) {
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/lib.rs"), "").unwrap();
    fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
}

fn cache(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("CACHEDIR.TAG"), TAG).unwrap();
    fs::write(path.join("artifact"), [b'x'; 8192]).unwrap();
}

fn cli(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-cleaner"));
    for key in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
    ] {
        command.env_remove(key);
    }
    if let Some(home) = dirs::home_dir() {
        command.env(
            "RUSTUP_HOME",
            std::env::var_os("RUSTUP_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".rustup")),
        );
    }
    command
        .current_dir(cwd)
        .env("HOME", home)
        .env("XDG_STATE_HOME", home)
        .env("CARGO_HOME", home.join("cargo"))
        .args(args)
        .output()
        .unwrap()
}

fn json(output: &Output, success: bool) -> Value {
    assert_eq!(
        output.status.success(),
        success,
        "{} {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cleans_a_workspace_once_and_keeps_sources() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let workspace = root.join("nested/workspace");
    let member = workspace.join("member");
    project(&member);
    fs::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nmembers = [\"member\"]\nresolver = \"3\"\n",
    )
    .unwrap();
    cache(&workspace.join("target"));

    let output = json(&cli(&root, &root, &["scan", "--path", ".", "--json"]), true);
    assert_eq!(output["projects"].as_array().unwrap().len(), 1);
    assert_eq!(output["projects"][0]["name"], "workspace");
    assert!(output["total_bytes"].as_u64().unwrap() > 0);

    let output = json(
        &cli(&root, &root, &["clean", ".", "--dry-run", "--json"]),
        true,
    );
    assert_eq!(output["caches"][0]["status"], "would_clean");
    assert!(workspace.join("target/artifact").exists());

    let output = json(&cli(&root, &root, &["clean", ".", "--json"]), true);
    assert_eq!(output["caches"].as_array().unwrap().len(), 1);
    assert_eq!(output["caches"][0]["status"], "cleaned");
    assert!(!workspace.join("target").exists());
    assert!(member.join("src/lib.rs").is_file());
}

#[test]
fn leaves_unmarked_linked_and_out_of_scope_targets_alone() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let scope = root.join("scope");
    let outside = root.join("outside");
    cache(&outside);

    let linked = scope.join("linked");
    project(&linked);
    symlink(&outside, linked.join("target")).unwrap();

    let unmarked = scope.join("unmarked");
    project(&unmarked);
    fs::create_dir(unmarked.join("target")).unwrap();
    fs::write(unmarked.join("target/notes"), "keep me").unwrap();

    let configured = scope.join("configured");
    project(&configured);
    fs::create_dir(configured.join(".cargo")).unwrap();
    fs::write(
        configured.join(".cargo/config.toml"),
        format!("[build]\ntarget-dir = \"{}\"\n", outside.display()),
    )
    .unwrap();

    let output = json(&cli(&scope, &root, &["clean", ".", "--json"]), false);
    assert_eq!(output["total_bytes"], 0);
    assert_eq!(output["caches"].as_array().unwrap().len(), 3);
    assert!(outside.join("artifact").is_file());
    assert!(unmarked.join("target/notes").is_file());
    assert!(linked.join("target").is_symlink());
}

#[test]
fn cleans_configured_build_and_target_directories_without_following_links() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let workspace = root.join("project");
    project(&workspace);
    fs::create_dir(workspace.join(".cargo")).unwrap();
    fs::write(
        workspace.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"output\"\nbuild-dir = \"intermediate\"\n",
    )
    .unwrap();
    cache(&workspace.join("output"));
    cache(&workspace.join("intermediate"));
    let notes = root.join("notes");
    fs::create_dir(&notes).unwrap();
    fs::write(notes.join("keep"), "outside the cache").unwrap();
    symlink(&notes, workspace.join("output/link")).unwrap();

    let output = json(&cli(&root, &root, &["clean", "./project", "--json"]), true);
    assert_eq!(output["caches"].as_array().unwrap().len(), 2);
    assert!(!workspace.join("output").exists());
    assert!(!workspace.join("intermediate").exists());
    assert!(notes.join("keep").is_file());
}

#[test]
fn rejects_a_cache_that_contains_the_project() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let workspace = root.join("project");
    project(&workspace);
    cache(&workspace);
    fs::create_dir(workspace.join(".cargo")).unwrap();
    fs::write(
        workspace.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \".\"\n",
    )
    .unwrap();

    let output = json(&cli(&root, &root, &["clean", "./project", "--json"]), false);
    assert_eq!(output["caches"][0]["status"], "skipped");
    assert!(workspace.join("src/lib.rs").is_file());
}

#[test]
fn prints_completions_and_rejects_short_flags() {
    let temp = tempfile::tempdir().unwrap();
    let complete = cli(temp.path(), temp.path(), &["complete", "--shell", "zsh"]);
    assert!(complete.status.success());
    assert!(String::from_utf8_lossy(&complete.stdout).starts_with("#compdef cargo-cleaner"));
    let short = cli(temp.path(), temp.path(), &["clean", "-h"]);
    assert!(!short.status.success());
}
