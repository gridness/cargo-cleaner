use crate::{
    git, managed, projects,
    ui::{self, Ui},
};
use clap::{Subcommand, ValueEnum};
use eyre::{Result, WrapErr, bail, ensure, eyre};
use serde::Serialize;
use std::{
    ffi::OsString,
    fs, io,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

#[derive(Subcommand)]
pub enum Action {
    /// Install a Git hook in the current repository
    Create {
        /// Git event that triggers cleanup
        #[arg(long, value_enum)]
        git: Option<Event>,
    },
    /// Show cargo-cleaner Git hooks in the current repository
    List,
    /// Remove a cargo-cleaner Git hook
    Remove {
        /// Git event to remove
        #[arg(long, value_enum)]
        git: Option<Event>,
    },
    /// Clean the current folder after a successful command
    Run {
        /// Program and arguments, following --
        #[arg(last = true, required = true, num_args = 1..)]
        command: Vec<OsString>,
    },
}

// ASVS 2.2.1: only supported Git event names become hook filenames.
#[derive(Clone, Copy, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    PostMerge,
    PostCheckout,
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Self::PostMerge => "post-merge",
            Self::PostCheckout => "post-checkout",
        }
    }

    fn script(self) -> String {
        let guard = match self {
            Self::PostMerge => "",
            Self::PostCheckout => {
                r#"[ "${3:-}" = 1 ] || exit 0
case "${1:-}" in
    *[!0]*) ;;
    *) exit 0 ;;
esac
"#
            }
        };
        // ASVS 1.2.5: the script consists solely of static shell text.
        format!(
            "#!/bin/sh\n# Installed by cargo-cleaner\n{guard}cargo-cleaner clean . >&2\nexit 0\n"
        )
    }
}

#[derive(Serialize)]
pub struct Hook {
    event: Event,
    path: PathBuf,
    pub active: bool,
}

struct Repository {
    dir: PathBuf,
    uses_default_hooks: bool,
}

impl Repository {
    fn open(root: &Path) -> Result<Self> {
        let common = git::common_dir(root)?;
        let dir = managed::directory(&common, Path::new("hooks"), false)?;
        Ok(Self {
            uses_default_hooks: git::hooks_dir(root)? == dir,
            dir,
        })
    }

    fn installed(&self) -> Result<Vec<Hook>> {
        let mut hooks = Vec::new();
        for &event in Event::value_variants() {
            let path = self.dir.join(event.name());
            if let Some(mode) = installed(event, &path)? {
                hooks.push(Hook {
                    event,
                    path,
                    active: self.uses_default_hooks && mode & 0o111 != 0,
                });
            }
        }
        Ok(hooks)
    }
}

fn installed(event: Event, path: &Path) -> Result<Option<u32>> {
    let script = event.script();
    let length = script.len() as u64;
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() == length => Ok(managed::read(path, length)?
            .filter(|contents| contents.bytes == script.as_bytes())
            .map(|contents| contents.mode)),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn inspect(root: &Path) -> Result<Vec<Hook>> {
    Repository::open(root)?.installed()
}

pub fn run(ui: &Ui, action: Action) -> Result<ExitCode> {
    let event = match action {
        Action::Run { command } => return after(ui, &command),
        Action::Create { git } | Action::Remove { git } => git,
        Action::List => None,
    };
    let root = git::root()?.ok_or_else(|| eyre!("Run this command inside a Git worktree"))?;
    let repository = Repository::open(&root)?;
    match action {
        Action::List => {
            let hooks = repository.installed()?;
            if ui.json {
                ui.json(&hooks)?;
            } else if hooks.is_empty() {
                println!("No cargo-cleaner Git hooks");
            } else {
                for hook in hooks {
                    let status = if hook.active { "active" } else { "inactive" };
                    println!("{}  {status}  {}", hook.event.name(), ui::path(&hook.path));
                }
            }
        }
        Action::Create { .. } => {
            ensure!(
                repository.uses_default_hooks,
                "Git uses a custom hooks directory. Add cargo-cleaner clean . to your hook there."
            );
            let event = choose(
                ui,
                event,
                Event::value_variants(),
                "Which Git event should trigger cleanup?",
            )?;
            let path = repository.dir.join(event.name());
            if let Some(mode) = installed(event, &path)? {
                ensure!(
                    mode & 0o111 != 0,
                    "The existing hook needs execute permissions: {}",
                    path.display()
                );
            } else {
                let common = repository.dir.parent().expect("Git hooks have a parent");
                managed::directory(common, Path::new("hooks"), true)?;
                managed::write(&path, event.script().as_bytes(), 0o755, None).wrap_err_with(
                    || {
                        format!(
                            "Could not install {}. An existing hook must be configured manually.",
                            path.display()
                        )
                    },
                )?;
            }
            report(ui, "Installed", event, &path)?;
        }
        Action::Remove { .. } => {
            let events = repository
                .installed()?
                .into_iter()
                .map(|hook| hook.event)
                .collect::<Vec<_>>();
            let event = choose(ui, event, &events, "Which Git hook should be removed?")?;
            let path = repository.dir.join(event.name());
            ensure!(
                installed(event, &path)?.is_some(),
                "No cargo-cleaner hook at {}",
                path.display()
            );
            fs::remove_file(&path)?;
            report(ui, "Removed", event, &path)?;
        }
        Action::Run { .. } => unreachable!(),
    }
    Ok(ExitCode::SUCCESS)
}

fn choose(ui: &Ui, event: Option<Event>, options: &[Event], prompt: &str) -> Result<Event> {
    if let Some(event) = event {
        return Ok(event);
    }
    match options {
        [] => bail!("No cargo-cleaner Git hooks"),
        [event] => Ok(*event),
        _ if ui.interactive() => {
            let labels = options
                .iter()
                .map(|event| event.name().to_owned())
                .collect::<Vec<_>>();
            Ok(options[ui::select(prompt, &labels)?])
        }
        _ => bail!("Use --git to choose a Git event"),
    }
}

fn report(ui: &Ui, action: &str, event: Event, path: &Path) -> Result<()> {
    if ui.json {
        ui.json(&serde_json::json!({ "event": event, "path": path }))?;
    } else {
        println!("{action} {} at {}", event.name(), ui::path(path));
    }
    Ok(())
}

fn after(ui: &Ui, args: &[OsString]) -> Result<ExitCode> {
    let (program, arguments) = args.split_first().expect("clap requires a command");
    // ASVS 1.2.5: pass the user's command and arguments directly to exec.
    let mut child = Command::new(program);
    child.args(arguments);
    if ui.json {
        child.stdout(Stdio::from(io::stderr()));
    }
    let status = child.status().wrap_err("Could not run the hook command")?;
    // ASVS 2.3.1: run cleanup only after the command succeeds.
    if !status.success() {
        let code = status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
        if ui.json {
            ui.json(&serde_json::json!({ "command_exit_code": code, "cleanup": "skipped" }))?;
        }
        return Ok(ExitCode::from(code as u8));
    }
    projects::clean(ui, Path::new("."), false).map(|ok| ExitCode::from(u8::from(!ok)))
}
