mod cron;
mod store;
mod time;

pub(crate) use self::store::Store;
use self::{cron::Cron, time::Time};
use crate::{
    projects,
    ui::{self, Ui},
};
use clap::{Args, Subcommand, ValueHint};
use cleaner_core::Roots;
use eyre::{Result, WrapErr, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
};

#[derive(Subcommand)]
pub enum Action {
    /// Add a recurring cleanup
    Create {
        /// When to run, for example "daily at 02:30" or "every Sunday at 02:30"
        #[arg(num_args = 0..)]
        time: Vec<String>,

        /// Search the whole system at each run
        #[arg(long, conflicts_with = "project")]
        all: bool,

        /// Clean this project by name or path. Repeat for multiple projects.
        #[arg(long, value_hint = ValueHint::DirPath)]
        project: Vec<PathBuf>,
    },
    /// Show saved schedules and their status
    List,
    /// Stop future runs and keep the schedule
    Stop(Selection),
    /// Remove a schedule
    Remove(Selection),
    #[command(hide = true)]
    Run {
        id: String,
        #[arg(long)]
        state: PathBuf,
    },
}

#[derive(Args)]
pub struct Selection {
    /// Select a schedule by ID
    #[arg(long, conflicts_with = "all")]
    id: Option<String>,

    /// Select all schedules
    #[arg(long)]
    all: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "paths", rename_all = "snake_case")]
enum Scope {
    System,
    Projects(Vec<PathBuf>),
}

impl Scope {
    fn description(&self) -> String {
        match self {
            Self::System => "all projects".to_owned(),
            Self::Projects(paths) => paths
                .iter()
                .map(|path| ui::path(path))
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Job {
    id: String,
    time: String,
    scope: Scope,
}

pub fn check_crontab() -> Result<()> {
    Cron::read().map(|_| ())
}

pub fn run(ui: &Ui, action: Action) -> Result<bool> {
    match action {
        Action::Create { time, all, project } => create(ui, &time.join(" "), all, project),
        Action::List => list(ui),
        Action::Stop(selection) => change(ui, selection, false),
        Action::Remove(selection) => change(ui, selection, true),
        Action::Run { id, state } => run_job(ui, &id, &state),
    }
}

fn create(ui: &Ui, time: &str, all: bool, project: Vec<PathBuf>) -> Result<bool> {
    ensure!(
        ui.interactive() || all || !project.is_empty(),
        "Use --all or --project to choose what to clean"
    );
    let time: Time = match time.parse() {
        Ok(time) => time,
        Err(_) if ui.interactive() => {
            ui::input("When should cleanup run? Try daily at 02:30.", time)?
        }
        Err(error) => bail!("{error}"),
    };
    let scope = if all {
        Scope::System
    } else if !project.is_empty() {
        let listing = if project.iter().any(|path| !projects::explicit_path(path)) {
            Some(projects::listing()?)
        } else {
            None
        };
        let mut paths = BTreeSet::new();
        for selector in project {
            let path = if projects::explicit_path(&selector) {
                projects::workspace_for_path(&selector)?
            } else {
                projects::project_path(
                    ui,
                    listing.as_ref().expect("names require an inventory"),
                    &selector,
                )?
            };
            paths.insert(path);
        }
        Scope::Projects(paths.into_iter().collect())
    } else {
        let choice = ui::select(
            "What should cleanup cover?",
            &["Selected projects".into(), "All projects".into()],
        )?;
        if choice == 1 {
            Scope::System
        } else {
            let listing = projects::listing()?;
            ensure!(!listing.projects.is_empty(), "No Cargo projects found");
            let choices = listing
                .projects
                .iter()
                .map(|project| ui::path(&project.path))
                .collect::<Vec<_>>();
            let selected = ui::multi(
                "Select projects with Space. Press Enter when you're done.",
                &choices,
            )?;
            Scope::Projects(
                selected
                    .iter()
                    .map(|index| listing.projects[*index].path.clone())
                    .collect(),
            )
        }
    };

    let executable = scheduled_executable()?;
    let store = Store::open()?;
    let _lock = store.lock("schedules.lock")?;
    let mut cron = Cron::read()?;
    let job = store.save(Job {
        id: String::new(),
        time: time.to_string(),
        scope,
    })?;
    if let Err(error) = cron
        .add(&time.cron(), &executable, &store.path, &job.id)
        .and_then(|()| cron.install())
    {
        store.remove(&job.id)?;
        return Err(error);
    }
    if ui.json {
        return ui.json(&job);
    }
    println!(
        "Scheduled {} {} for {}.",
        job.id,
        job.time,
        job.scope.description()
    );
    Ok(true)
}

fn scheduled_executable() -> Result<PathBuf> {
    let executable = env::current_exe().wrap_err("Could not find the cargo-cleaner executable")?;
    let canonical = executable.canonicalize()?;
    let path = env::var_os("PATH").unwrap_or_default();
    Ok(env::split_paths(&path)
        .filter(|path| path.is_absolute())
        .map(|dir| dir.join("cargo-cleaner"))
        .find(|path| path.canonicalize().is_ok_and(|path| path == canonical))
        .unwrap_or(executable))
}

fn list(ui: &Ui) -> Result<bool> {
    #[derive(Serialize)]
    struct Entry<'a> {
        #[serde(flatten)]
        job: &'a Job,
        active: bool,
    }
    let store = Store::open()?;
    let _lock = store.lock("schedules.lock")?;
    let jobs = store.jobs()?;
    if jobs.is_empty() {
        if ui.json {
            return ui.json(&jobs);
        }
        println!("No cleanup schedules.");
        return Ok(true);
    }
    let cron = Cron::read()?;
    let entries = jobs
        .iter()
        .map(|job| Entry {
            job,
            active: cron.contains(&job.id),
        })
        .collect::<Vec<_>>();
    if ui.json {
        return ui.json(&entries);
    }
    for entry in entries {
        let status = if entry.active {
            console::style("Active").green()
        } else {
            console::style("Stopped").yellow()
        };
        let job = entry.job;
        println!(
            "{}  {}  {}  {}",
            job.id,
            status,
            ui::safe(&job.time),
            job.scope.description()
        );
    }
    Ok(true)
}

fn change(ui: &Ui, selection: Selection, remove: bool) -> Result<bool> {
    let store = Store::open()?;
    // Resolve prompts before locking so a menu cannot block scheduled runs.
    let jobs = store.jobs()?;
    let ids = if let Some(id) = selection.id {
        ensure!(
            jobs.iter().any(|job| job.id == id),
            "No schedule with that ID. Run cargo-cleaner schedule list."
        );
        vec![id]
    } else if selection.all || jobs.len() <= 1 {
        jobs.into_iter().map(|job| job.id).collect()
    } else if ui.interactive() {
        let options = jobs
            .iter()
            .map(|job| {
                format!(
                    "{}  {}  {}",
                    job.id,
                    ui::safe(&job.time),
                    job.scope.description()
                )
            })
            .collect::<Vec<_>>();
        let prompt = if remove {
            "Which schedule should be removed?"
        } else {
            "Which schedule should stop?"
        };
        vec![jobs[ui::select(prompt, &options)?].id.clone()]
    } else {
        bail!("Use --id or --all to choose a schedule. Run cargo-cleaner schedule list for IDs.");
    };
    if !ids.is_empty() {
        let _lock = store.lock("schedules.lock")?;
        let mut cron = Cron::read()?;
        if ids.iter().any(|id| cron.contains(id)) {
            cron.remove(&ids);
            cron.install()?;
        }
        if remove {
            for id in &ids {
                store.remove(id)?;
            }
        }
    }
    if ui.json {
        return ui.json(&ids);
    }
    let verb = if remove { "Removed" } else { "Stopped" };
    if ids.is_empty() {
        println!("No cleanup schedules.");
    } else {
        println!("{verb} {}.", ids.join(", "));
    }
    Ok(true)
}

fn run_job(ui: &Ui, id: &str, state: &Path) -> Result<bool> {
    let store = Store::at(state)?;
    let lock = store.lock("schedules.lock")?;
    if !Cron::read()?.contains(id) {
        return Ok(true);
    }
    let Some(job) = store.jobs()?.into_iter().find(|job| job.id == id) else {
        return Ok(true);
    };
    drop(lock);
    let roots = match job.scope {
        Scope::System => Roots::System,
        Scope::Projects(paths) => Roots::Projects(paths),
    };
    projects::clean_roots(ui, &roots, false, Some(&store))
}
