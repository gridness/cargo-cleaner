mod harness;

use crate::{
    git, managed,
    schedule::Store,
    ui::{self, Ui},
};
use clap::{Args, Subcommand, ValueHint};
use dialoguer::Confirm;
use eyre::{Result, WrapErr, bail, ensure, eyre};
use serde::Serialize;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const SKILL: &[u8] = include_bytes!("SKILL.md");
const SKILL_DIR: &str = "cargo-cleaner";
const SKILL_FILE: &str = "SKILL.md";
const AGENTS_BEGIN: &str = "<!-- cargo-cleaner:begin -->";
const AGENTS_END: &str = "<!-- cargo-cleaner:end -->";
const AGENTS_BLOCK: &str = concat!(
    "<!-- cargo-cleaner:begin -->\n",
    "When asked to reclaim Cargo build space, load the `cargo-cleaner` skill.\n",
    "<!-- cargo-cleaner:end -->\n"
);
const MAX_MARKDOWN: u64 = 4 * 1024 * 1024;

#[derive(Subcommand)]
pub enum Action {
    /// Install the bundled skill
    Install {
        #[command(flatten)]
        target: Target,

        /// Add a skill pointer to the project's AGENTS.md
        #[arg(long, requires = "local")]
        add_agents_entry: bool,

        /// Answer yes to the AGENTS.md prompt
        #[arg(long, requires = "local")]
        yes: bool,
    },
    /// Remove the bundled skill
    Remove {
        #[command(flatten)]
        target: Target,

        /// Remove the managed entry from the project's AGENTS.md
        #[arg(long)]
        remove_agents_entry: bool,
    },
}

#[derive(Args)]
pub struct Target {
    /// Use the current project instead of the user's global directories
    #[arg(long)]
    local: bool,

    /// Select one or more harnesses
    #[arg(long, num_args = 1.., value_parser = harness::parser())]
    harness: Vec<String>,

    /// Use an existing custom skills directory
    #[arg(long, conflicts_with = "harness", value_hint = ValueHint::DirPath)]
    skills_dir: Option<PathBuf>,
}

pub fn project_root() -> Result<PathBuf> {
    Ok(git::root()?.map_or_else(env::current_dir, Ok)?)
}

pub fn inspect(root: Option<&Path>) -> Result<Vec<PathBuf>> {
    let home = dirs::home_dir().ok_or_else(|| eyre!("Could not find your home directory"))?;
    Ok(installed(&home, false)
        .into_iter()
        .chain(root.into_iter().flat_map(|root| installed(root, true)))
        .collect())
}

fn installed(root: &Path, local: bool) -> Vec<PathBuf> {
    harness::directories(&[], local, false, root, None)
        .into_iter()
        .filter_map(|relative| skill_file(root, relative, false).ok())
        .filter(|path| {
            managed::read(path, SKILL.len() as u64)
                .is_ok_and(|contents| contents.is_some_and(|file| file.bytes == SKILL))
        })
        .collect()
}

pub fn run(ui: &Ui, action: Action) -> Result<bool> {
    let (install, target, agents) = match action {
        Action::Install {
            target,
            add_agents_entry,
            yes,
        } => {
            let agents = target.local
                && (add_agents_entry
                    || yes
                    || (ui.interactive()
                        && Confirm::new()
                            .with_prompt("Add the cargo-cleaner skill pointer to AGENTS.md?")
                            .default(false)
                            .interact()
                            .wrap_err("Could not read the answer")?));
            (true, target, agents)
        }
        Action::Remove {
            target,
            remove_agents_entry,
        } => (false, target, remove_agents_entry),
    };

    let project = (target.local || agents).then(project_root).transpose()?;
    let home = dirs::home_dir();
    let custom = target
        .skills_dir
        .as_ref()
        .map(fs::canonicalize)
        .transpose()?;
    let root = if let Some(custom) = &custom {
        ensure!(
            custom.is_dir(),
            "The custom skills path must be a directory"
        );
        custom
    } else if target.local {
        project.as_ref().expect("local mode has a project")
    } else {
        home.as_ref()
            .ok_or_else(|| eyre!("Could not find your home directory"))?
    };

    ensure!(root.is_absolute(), "The skills directory must be absolute");
    let store = Store::open()?;
    let _lock = store.lock("skill.lock")?;
    let agents_update = if agents {
        agents_update(
            project.as_deref().expect("AGENTS.md has a project"),
            install,
        )?
    } else {
        None
    };
    let paths = if custom.is_some() {
        vec![Path::new("")]
    } else if !install && target.harness.is_empty() {
        Vec::new()
    } else {
        harness::directories(
            &target.harness,
            target.local,
            install,
            root,
            home.as_deref(),
        )
        .into_iter()
        .collect()
    };

    let mut changes = Vec::new();
    for relative in paths {
        let path = skill_file(root, relative, false)?;
        let exists = match managed::read(&path, MAX_MARKDOWN)? {
            Some(file) if file.bytes == SKILL => true,
            Some(_) => bail!("A different skill already exists at {}", path.display()),
            None => false,
        };
        if install != exists {
            changes.push((relative, path));
        }
    }
    if !install && custom.is_none() && target.harness.is_empty() {
        changes.extend(
            installed(root, target.local)
                .into_iter()
                .map(|path| (Path::new(""), path)),
        );
    }

    let mut paths = Vec::new();
    for (relative, path) in changes {
        if install {
            skill_file(root, relative, true)?;
            managed::write(&path, SKILL, 0o644, None)?;
        } else {
            fs::remove_file(&path)?;
            let dir = path.parent().expect("the skill file has a directory");
            match fs::remove_dir(dir) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
                Err(error) => return Err(error.into()),
            }
        }
        paths.push(path);
    }
    let agents_path = agents_update
        .map(|AgentsUpdate { path, old, new }| {
            managed::write(&path, &new, 0o644, old.as_ref())?;
            Ok::<_, eyre::Report>(path)
        })
        .transpose()?;

    #[derive(Serialize)]
    struct Report {
        skills: Vec<PathBuf>,
        agents_entry: Option<PathBuf>,
    }
    if ui.json {
        return ui.json(&Report {
            skills: paths,
            agents_entry: agents_path,
        });
    }
    let verb = if install { "Installed" } else { "Removed" };
    if paths.is_empty() && agents_path.is_none() {
        println!("No changes");
    }
    for path in paths {
        println!("{verb} {}", ui::path(&path));
    }
    if let Some(path) = agents_path {
        println!("{verb} skill pointer in {}", ui::path(&path));
    }
    Ok(true)
}

fn skill_file(root: &Path, relative: &Path, create: bool) -> Result<PathBuf> {
    Ok(managed::directory(root, &relative.join(SKILL_DIR), create)?.join(SKILL_FILE))
}

struct AgentsUpdate {
    path: PathBuf,
    old: Option<managed::Contents>,
    new: Vec<u8>,
}

fn agents_update(root: &Path, install: bool) -> Result<Option<AgentsUpdate>> {
    let path = root.join("AGENTS.md");
    let old = managed::read(&path, MAX_MARKDOWN)?;
    let text = old
        .as_ref()
        .map_or(Ok(""), |file| std::str::from_utf8(&file.bytes))
        .wrap_err("AGENTS.md must be UTF-8")?;
    let start = text.find(AGENTS_BLOCK);
    let expected_count = usize::from(start.is_some());
    ensure!(
        text.matches(AGENTS_BEGIN).count() == expected_count
            && text.matches(AGENTS_END).count() == expected_count,
        "The cargo-cleaner entry in AGENTS.md has been edited"
    );

    let new = match (install, start) {
        (true, None) => {
            let separator = if text.is_empty() { "" } else { "\n\n" };
            format!("{text}{separator}{AGENTS_BLOCK}").into_bytes()
        }
        (false, Some(start)) => {
            let end = start + AGENTS_BLOCK.len();
            let before = &text[..start];
            let after = &text[end..];
            let before = if after.is_empty() {
                before.strip_suffix("\n\n").unwrap_or(before)
            } else {
                before
            };
            [before.as_bytes(), after.as_bytes()].concat()
        }
        _ => return Ok(None),
    };
    ensure!(
        new.len() as u64 <= MAX_MARKDOWN,
        "AGENTS.md exceeds the size limit"
    );
    Ok(Some(AgentsUpdate { path, old, new }))
}
