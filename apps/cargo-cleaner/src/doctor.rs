use crate::{
    git, hooks, schedule, skill,
    ui::{self, Ui},
};
use cleaner_core::Cargo;
use eyre::Result;
use serde::Serialize;
use std::{env, path::Path};

#[derive(Serialize)]
struct Check {
    name: &'static str,
    status: &'static str,
    detail: String,
}

impl Check {
    fn probe(name: &'static str, result: Result<String>, failure: &'static str) -> Self {
        match result {
            Ok(detail) => Self {
                name,
                status: "ok",
                detail,
            },
            Err(error) => Self {
                name,
                status: failure,
                detail: format!("{error:#}"),
            },
        }
    }
}

pub fn run(ui: &Ui) -> Result<bool> {
    let cargo = Cargo::find()
        .and_then(|cargo| cargo.version())
        .map_err(|error| eyre::eyre!("{error:#}"));
    let repo = git::root();
    let project = repo.as_ref().ok().and_then(|repo| repo.as_deref());
    let cwd = env::current_dir()?;
    let checks = [
        Check::probe("Cargo", cargo, "error"),
        Check::probe("Git", git::version(), "warning"),
        Check::probe(
            "Scheduler",
            schedule::check_crontab().map(|()| "User crontab readable".to_owned()),
            "warning",
        ),
        match repo.as_ref() {
            Ok(Some(root)) => Check::probe(
                "Git hooks",
                hooks::inspect(root).map(|hooks| {
                    let active = hooks.iter().filter(|hook| hook.active).count();
                    format!("{active} active, {} inactive", hooks.len() - active)
                }),
                "warning",
            ),
            Ok(None) => Check {
                name: "Git hooks",
                status: "skipped",
                detail: "Outside a Git worktree".to_owned(),
            },
            Err(error) => Check {
                name: "Git hooks",
                status: "warning",
                detail: format!("{error:#}"),
            },
        },
        Check::probe(
            "Agent skill",
            skill::inspect(project.or(Some(&cwd))).map(|paths| {
                if paths.is_empty() {
                    "No installed copies in known skill directories".to_owned()
                } else {
                    paths
                        .iter()
                        .map(|path| Path::display(path).to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            }),
            "warning",
        ),
    ];

    let success = checks.iter().all(|check| check.status != "error");
    for check in &checks {
        ui.explain(
            None,
            format!("{}: {}. {}", check.name, check.status, check.detail),
        );
    }
    if ui.json {
        ui.json(&checks)?;
    } else {
        for check in checks {
            println!(
                "{:<12} {:<7} {}",
                check.name,
                check.status,
                ui::safe(&check.detail)
            );
        }
    }
    Ok(success)
}
