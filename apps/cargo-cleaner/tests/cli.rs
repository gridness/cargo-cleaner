use serde_json::Value;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
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
fn explains_skipped_manifests_from_saved_results_without_rescanning() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let broken = root.join("broken\u{1b}[2J\nproject");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join("Cargo.toml"), "[package\ninvalid").unwrap();

    let scan = json(
        &isolated(env!("CARGO_BIN_EXE_cargo-cleaner"), &root, &root)
            .env("CARGO_TERM_COLOR", "always")
            .args(["scan", "--path", ".", "--json"])
            .output()
            .unwrap(),
        true,
    );
    assert_eq!(scan["unreadable_manifests"], 1);
    assert_eq!(
        scan["issues"][0]["path"],
        broken.join("Cargo.toml").to_str().unwrap()
    );
    assert!(
        scan["issues"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("error:")
    );
    fs::remove_dir_all(&broken).unwrap();

    let first = json(&cli(&root, &root, &["explain", "--json"]), true);
    assert_eq!(first["command"], "cargo-cleaner scan");
    let entries = first["entries"].to_string();
    assert!(entries.contains("Cargo.toml"));
    assert!(entries.contains("error:"));
    assert!(entries.contains("Skipped 1 Cargo manifest."));
    let text = cli(&root, &root, &["explain"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(!text.contains('\u{1b}'));
    assert!(text.contains("\\u{1b}[2J\\nproject"));
    assert!(cli(&root, &root, &["--help"]).status.success());
    assert!(
        cli(&root, &root, &["complete", "--shell", "zsh"])
            .status
            .success()
    );
    let again = json(&cli(&root, &root, &["explain", "--json"]), true);
    assert_eq!(first, again);
}

#[test]
fn explains_cleanup_safety_dry_runs_and_the_latest_failure() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let workspace = root.join("workspace");
    project(&workspace);
    fs::create_dir(workspace.join("target")).unwrap();
    fs::write(workspace.join("target/notes"), "keep me").unwrap();
    json(&cli(&workspace, &root, &["clean", ".", "--json"]), false);
    let report = json(&cli(&root, &root, &["explain", "--json"]), true);
    assert_eq!(report["command"], "cargo-cleaner clean");
    assert_eq!(report["success"], false);
    assert!(
        report["entries"]
            .to_string()
            .contains("Do not add a cache marker")
    );
    assert_eq!(
        fs::read_to_string(workspace.join("target/notes")).unwrap(),
        "keep me"
    );

    cache(&workspace.join("target"));
    json(
        &cli(&workspace, &root, &["clean", ".", "--dry-run", "--json"]),
        true,
    );
    let report = json(&cli(&root, &root, &["explain", "--json"]), true);
    assert!(
        report["entries"]
            .to_string()
            .contains("dry run did not delete")
    );
    assert!(workspace.join("target/artifact").is_file());

    json(&cli(&workspace, &root, &["clean", ".", "--json"]), true);
    let report = json(&cli(&root, &root, &["explain", "--json"]), true);
    assert!(
        report["entries"]
            .to_string()
            .contains("Project sources were kept")
    );
    assert!(workspace.join("src/lib.rs").is_file());

    let failure = cli(&root, &root, &["scan", "--path", "missing", "--json"]);
    assert!(!failure.status.success());
    assert!(failure.stdout.is_empty());
    let report = json(&cli(&root, &root, &["explain", "--json"]), true);
    assert_eq!(report["command"], "cargo-cleaner scan");
    assert_eq!(report["success"], false);
    assert!(
        report["entries"]
            .to_string()
            .contains("Could not open the scan folder")
    );
}

#[test]
fn explains_unreadable_folders_and_empty_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let locked = root.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o0)).unwrap();
    let result = cli(&root, &root, &["clean", ".", "--json"]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    let result = json(&result, false);
    assert_eq!(result["unreadable_entries"], 1);
    let report = json(&cli(&root, &root, &["explain", "--json"]), true);
    let entries = report["entries"].to_string();
    assert!(entries.contains("Permission denied"));
    assert!(entries.contains(locked.to_str().unwrap()));
    assert!(entries.contains("No readable Cargo workspaces were found"));
}

#[test]
fn explanation_state_is_private_bounded_and_kept_separate_from_schedules() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    assert_eq!(
        json(&cli(&root, &root, &["explain", "--json"]), true),
        Value::Null
    );
    assert!(
        String::from_utf8(cli(&root, &root, &["explain"]).stdout)
            .unwrap()
            .contains("No previous command")
    );
    json(&cli(&root, &root, &["scan", "--path", ".", "--json"]), true);
    let before = json(&cli(&root, &root, &["explain", "--json"]), true);
    let state = if cfg!(target_os = "macos") {
        root.join("Library/Application Support/cargo-cleaner")
    } else {
        root.join("cargo-cleaner")
    };
    let report = state.join("last-command.report");
    assert_eq!(
        fs::metadata(&report).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let _scheduled = cli(
        &root,
        &root,
        &[
            "schedule",
            "run",
            "invalid",
            "--state",
            state.to_str().unwrap(),
        ],
    );
    assert_eq!(
        json(&cli(&root, &root, &["explain", "--json"]), true),
        before
    );
    assert_eq!(
        json(&cli(&root, &root, &["schedule", "list", "--json"]), true),
        serde_json::json!([])
    );

    fs::write(&report, "x".repeat(1024 * 1024 + 1)).unwrap();
    assert!(!cli(&root, &root, &["explain"]).status.success());
    fs::write(&report, "invalid json").unwrap();
    assert!(!cli(&root, &root, &["explain"]).status.success());
    fs::remove_file(&report).unwrap();
    let outside = root.join("untouched");
    fs::write(&outside, "keep").unwrap();
    symlink(&outside, &report).unwrap();
    assert!(!cli(&root, &root, &["explain"]).status.success());
    json(&cli(&root, &root, &["scan", "--path", ".", "--json"]), true);
    assert!(!report.is_symlink());
    assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
}

#[test]
fn bounds_skip_samples_without_losing_counts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("Cargo.toml"), "[package\ninvalid").unwrap();
    let paths = (0..105)
        .map(|i| root.join(format!("locked-{i}")))
        .collect::<Vec<_>>();
    for path in &paths {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o0)).unwrap();
    }
    let result = cli(&root, &root, &["scan", "--path", ".", "--json"]);
    for path in &paths {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let result = json(&result, true);
    assert_eq!(result["unreadable_entries"], 105);
    assert_eq!(result["unreadable_manifests"], 1);
    assert_eq!(result["issues"].as_array().unwrap().len(), 101);
    let report = json(&cli(&root, &root, &["explain", "--json"]), true);
    assert_eq!(report["omitted"], 5);
    assert!(report["entries"].to_string().contains("unclosed table"));
    assert!(
        report["entries"]
            .to_string()
            .contains("Skipped 105 filesystem entries")
    );
    let text = cli(&root, &root, &["explain"]);
    assert!(
        String::from_utf8(text.stdout)
            .unwrap()
            .contains("5 more details were omitted")
    );
}

#[test]
fn saving_an_explanation_never_changes_command_status_or_json() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let state = if cfg!(target_os = "macos") {
        root.join("Library/Application Support/cargo-cleaner")
    } else {
        root.join("cargo-cleaner")
    };
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    let outside = root.join("outside-state");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, &state).unwrap();
    let result = cli(&root, &root, &["scan", "--path", ".", "--json"]);
    assert_eq!(json(&result, true)["projects"], serde_json::json!([]));
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("Could not save the explanation")
    );
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

#[test]
fn schedule_logs_selects_saved_jobs_and_keeps_json_and_terminal_output_safe() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let state = if cfg!(target_os = "macos") {
        root.join("Library/Application Support/cargo-cleaner")
    } else {
        root.join("cargo-cleaner")
    };
    let logs = || cli(&root, &root, &["schedule", "logs", "--json"]);
    let empty = json(&logs(), true);
    assert!(empty["id"].is_null());
    assert_eq!(empty["runs"], serde_json::json!([]));
    assert!(state.is_dir());

    let save_job = |id| {
        fs::write(
            state.join(format!("{id}.json")),
            serde_json::to_vec(&serde_json::json!({
                "id": id,
                "time": "daily at 02:30",
                "scope": {"type": "system"}
            }))
            .unwrap(),
        )
        .unwrap();
    };
    save_job("job-123");
    let empty = json(&logs(), true);
    assert_eq!(empty["id"], "job-123");
    assert_eq!(empty["runs"], serde_json::json!([]));
    let no_runs = cli(&root, &root, &["schedule", "logs"]);
    assert!(no_runs.status.success());
    assert!(String::from_utf8_lossy(&no_runs.stdout).contains("No recorded runs"));

    let runs = serde_json::json!([
        {
            "id": "run-first", "status": "succeeded",
            "started_at_unix_secs": 100, "finished_at_unix_secs": 101,
            "duration_ms": 1000, "stdout": "Cleaned project\n", "stderr": "",
            "stdout_truncated": false, "stderr_truncated": false, "error": null
        },
        {
            "id": "run-second", "status": "failed",
            "started_at_unix_secs": 200, "finished_at_unix_secs": 201,
            "duration_ms": 1000, "stdout": "output\u{1b}[31m\nnext line\n",
            "stderr": "warning\rhidden\n", "stdout_truncated": true,
            "stderr_truncated": false, "error": "failure\u{1b}[2J"
        }
    ]);
    let history = state.join("job-123.logs");
    fs::write(&history, serde_json::to_vec(&runs).unwrap()).unwrap();
    fs::set_permissions(&history, fs::Permissions::from_mode(0o600)).unwrap();
    let result = json(&logs(), true);
    assert_eq!(result["runs"][0], runs[1]);
    assert_eq!(result["runs"][1], runs[0]);
    let text = cli(&root, &root, &["schedule", "logs"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.find("Failed").unwrap() < text.find("Succeeded").unwrap());
    assert!(!text.contains('\u{1b}'));
    assert!(!text.contains('\r'));
    assert!(text.contains("\\u{1b}[31m"));
    assert!(text.contains("stdout truncated at 64 KiB"));

    save_job("job-456");
    let ambiguous = logs();
    assert!(!ambiguous.status.success());
    assert!(ambiguous.stdout.is_empty());
    let error = String::from_utf8_lossy(&ambiguous.stderr);
    for hint in ["--id", "job-123", "job-456", "schedule list"] {
        assert!(error.contains(hint));
    }
    let explicit = cli(
        &root,
        &root,
        &["schedule", "logs", "--id", "job-456", "--json"],
    );
    assert_eq!(json(&explicit, true)["id"], "job-456");
    let unknown = cli(
        &root,
        &root,
        &["schedule", "logs", "--id", "../other", "--json"],
    );
    assert!(!unknown.status.success());
    assert!(unknown.stdout.is_empty());
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
