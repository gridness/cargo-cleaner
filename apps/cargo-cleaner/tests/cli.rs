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

fn isolated(program: &str, cwd: &Path, home: &Path) -> Command {
    let mut command = Command::new(program);
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
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("XDG_STATE_HOME", home)
        .env("CARGO_HOME", home.join("cargo"));
    command
}

fn cli(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    isolated(env!("CARGO_BIN_EXE_cargo-cleaner"), cwd, home)
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

#[test]
fn command_hook_cleans_only_after_success_and_keeps_child_status() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let workspace = root.join("workspace");
    let outside = root.join("other");
    project(&workspace);
    project(&outside);
    cache(&workspace.join("target"));
    cache(&outside.join("target"));

    let failure = cli(
        &workspace,
        &root,
        &[
            "hooks",
            "run",
            "--json",
            "--",
            "sh",
            "-c",
            "echo child; exit 7",
        ],
    );
    assert_eq!(failure.status.code(), Some(7));
    assert_eq!(json(&failure, false)["cleanup"], "skipped");
    assert_eq!(failure.stderr, b"child\n");
    assert!(workspace.join("target/artifact").is_file());

    let success = cli(
        &workspace,
        &root,
        &[
            "hooks", "run", "--json", "--", "test", "x; false", "=", "x; false",
        ],
    );
    assert_eq!(json(&success, true)["caches"][0]["status"], "cleaned");
    assert!(!workspace.join("target").exists());
    assert!(workspace.join("src/lib.rs").is_file());
    assert!(outside.join("target/artifact").is_file());
}

#[test]
fn git_hooks_run_in_the_worktree_and_preserve_existing_hooks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("worktree with spaces");
    project(&repo);

    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    symlink(
        env!("CARGO_BIN_EXE_cargo-cleaner"),
        bin.join("cargo-cleaner"),
    )
    .unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path))).unwrap();
    let git = |args: &[&str]| {
        let output = isolated("git", &repo, &root)
            .env("PATH", &path)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--initial-branch=main"]);
    git(&["add", "Cargo.toml", "src/lib.rs"]);
    git(&["commit", "-m", "Initial"]);
    git(&["switch", "-c", "feature"]);
    git(&["commit", "--allow-empty", "-m", "Feature"]);
    git(&["switch", "main"]);

    let create = |event| {
        cli(
            &repo.join("src"),
            &root,
            &["hooks", "create", "--git", event, "--json"],
        )
    };
    let list = || cli(&repo, &root, &["hooks", "list", "--json"]);
    let hook = repo.join(".git/hooks/post-merge");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    assert!(!create("post-merge").status.success());
    assert_eq!(fs::read_to_string(&hook).unwrap(), "#!/bin/sh\nexit 1\n");
    fs::remove_file(&hook).unwrap();

    assert_eq!(json(&create("post-merge"), true)["event"], "post-merge");
    assert_eq!(
        json(&create("post-checkout"), true)["event"],
        "post-checkout"
    );
    assert_eq!(json(&list(), true).as_array().unwrap().len(), 2);

    cache(&repo.join("target"));
    git(&["checkout", "--", "Cargo.toml"]);
    assert!(repo.join("target/artifact").is_file());
    git(&["merge", "feature"]);
    assert!(!repo.join("target").exists());

    cache(&repo.join("target"));
    git(&["switch", "feature"]);
    assert!(!repo.join("target").exists());
    assert!(repo.join("src/lib.rs").is_file());

    git(&["config", "core.hooksPath", ".githooks"]);
    assert_eq!(json(&list(), true)[0]["active"], false);
    assert!(!create("post-merge").status.success());
    assert!(!repo.join(".githooks").exists());
    assert!(
        cli(&repo, &root, &["hooks", "remove", "--git", "post-merge"])
            .status
            .success()
    );
    assert!(!hook.exists());
    assert_eq!(json(&list(), true).as_array().unwrap().len(), 1);
}

#[test]
fn installs_shared_skills_once_and_round_trips_agents_entry() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    project(&repo);
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    let agents = repo.join("AGENTS.md");
    let original = b"# Project\r\nUse Cargo.";
    fs::write(&agents, original).unwrap();

    let install = || {
        cli(
            &repo.join("src"),
            &root,
            &[
                "skill",
                "install",
                "--harness",
                "codex",
                "cursor",
                "claude-code",
                "--local",
                "--add-agents-entry",
                "--yes",
                "--json",
            ],
        )
    };
    let result = json(&install(), true);
    assert_eq!(result["skills"].as_array().unwrap().len(), 2);
    for path in [".agents/skills", ".claude/skills"] {
        let skill = fs::read_to_string(repo.join(path).join("cargo-cleaner/SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: cargo-cleaner\n"));
        assert!(skill.contains("cargo-cleaner clean . --dry-run"));
    }
    let updated = fs::read(&agents).unwrap();
    assert!(updated.starts_with(original));
    assert!(updated.ends_with(b"<!-- cargo-cleaner:end -->\n"));
    assert_eq!(json(&install(), true)["skills"], serde_json::json!([]));
    assert_eq!(fs::read(&agents).unwrap(), updated);

    let report = json(&cli(&repo, &root, &["doctor", "--json"]), true);
    let report = report.as_array().unwrap();
    assert_eq!(report[0]["name"], "Cargo");
    assert_eq!(report[0]["status"], "ok");
    assert!(report.iter().any(|check| check["name"] == "Agent skill"
        && check["detail"].as_str().unwrap().contains(".claude/skills")));

    let result = json(
        &cli(
            &repo,
            &root,
            &[
                "skill",
                "remove",
                "--local",
                "--remove-agents-entry",
                "--json",
            ],
        ),
        true,
    );
    assert_eq!(result["skills"].as_array().unwrap().len(), 2);
    assert!(!repo.join(".agents/skills/cargo-cleaner").exists());
    assert!(!repo.join(".claude/skills/cargo-cleaner").exists());
    assert_eq!(fs::read(&agents).unwrap(), original);
}

#[test]
fn global_skill_install_and_remove_preserve_foreign_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let skill = root.join(".dsh/skills/cargo-cleaner/SKILL.md");
    let args = ["skill", "install", "--harness", "deepseek", "--json"];
    let install = || cli(&project, &root, &args);
    assert_eq!(
        json(&install(), true)["skills"].as_array().unwrap().len(),
        1
    );
    assert!(skill.is_file());

    let foreign = root.join(".pi/agent/skills/cargo-cleaner/SKILL.md");
    fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    fs::write(&foreign, "other skill").unwrap();
    let result = json(&cli(&project, &root, &["skill", "remove", "--json"]), true);
    assert_eq!(result["skills"].as_array().unwrap().len(), 1);
    assert!(!skill.exists());
    assert_eq!(fs::read_to_string(&foreign).unwrap(), "other skill");

    assert!(
        !cli(
            &project,
            &root,
            &["skill", "install", "--harness", "deepseek", "pi"]
        )
        .status
        .success()
    );
    assert!(!skill.exists());

    symlink(&root, project.join(".agents")).unwrap();
    assert!(
        !cli(
            &project,
            &root,
            &["skill", "install", "--harness", "codex", "--local"]
        )
        .status
        .success()
    );
    assert!(!root.join("skills").exists());
}
