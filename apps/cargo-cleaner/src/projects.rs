use crate::{
    schedule::Store,
    ui::{self, Ui},
};
use cleaner_core::{Cargo, Cleaner, Inventory, Roots};
use eyre::{Result, WrapErr, bail, eyre};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

fn cargo() -> Result<Cargo> {
    Cargo::find().map_err(|error| eyre!("{error:#}"))
}

fn inventory(cargo: &Cargo, roots: &Roots) -> Result<Inventory> {
    cleaner_core::scan(cargo, roots).map_err(|error| eyre!("{error:#}"))
}

pub fn scan(ui: &Ui, path: Option<PathBuf>) -> Result<bool> {
    let roots = path.map_or(Roots::System, Roots::Directory);
    let inventory = inventory(&cargo()?, &roots)?;
    if ui.json {
        #[derive(Serialize)]
        struct Output<'a> {
            total_bytes: u64,
            #[serde(flatten)]
            inventory: &'a Inventory,
        }
        return ui.json(&Output {
            total_bytes: inventory.bytes(),
            inventory: &inventory,
        });
    }

    let caches: BTreeMap<_, _> = inventory
        .caches
        .iter()
        .map(|cache| (&cache.path, cache))
        .collect();
    let mut rows = inventory
        .projects
        .iter()
        .map(|project| {
            let bytes = project
                .caches
                .iter()
                .filter_map(|path| caches.get(path))
                .fold(0u64, |total, cache| total.saturating_add(cache.bytes));
            (bytes, project)
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|(bytes, project)| (std::cmp::Reverse(*bytes), &project.path));
    for (bytes, project) in rows {
        println!(
            "{:>10}  {}  {}",
            ui::size(bytes),
            console::style(ui::safe(&project.name)).cyan(),
            ui::path(&project.path)
        );
    }
    println!(
        "{} projects, {} in build caches",
        inventory.projects.len(),
        ui::size(inventory.bytes())
    );
    for cache in &inventory.caches {
        if let Some(issue) = &cache.issue {
            ui.warn(&format!("{} ({issue})", ui::path(&cache.path)));
        }
    }
    report_skips(ui, &inventory);
    Ok(true)
}

pub fn project_path(ui: &Ui, listing: &Inventory, name: &Path) -> Result<PathBuf> {
    let matches = listing
        .projects
        .iter()
        .filter(|project| project.path.file_name() == Some(name.as_os_str()))
        .map(|project| &project.path)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => bail!(
            "No project named \"{}\". Run cargo-cleaner scan to see project names.",
            ui::path(name)
        ),
        [path] => Ok((*path).clone()),
        paths => {
            let labels = paths.iter().map(|path| ui::path(path)).collect::<Vec<_>>();
            if !ui.interactive() {
                bail!(
                    "Several projects are called \"{}\". Use an exact folder path from cargo-cleaner scan.",
                    ui::path(name)
                );
            }
            Ok(paths[ui::select("Which project?", &labels)?].clone())
        }
    }
}

pub fn explicit_path(path: &Path) -> bool {
    path.is_absolute() || path.as_os_str().as_encoded_bytes().contains(&b'/')
}

pub fn workspace_for_path(path: &Path) -> Result<PathBuf> {
    cargo()?.workspace(path).map_err(|error| eyre!("{error:#}"))
}

pub fn listing() -> Result<Inventory> {
    inventory(&cargo()?, &Roots::System)
}

pub fn clean(ui: &Ui, project: &Path, dry_run: bool) -> Result<bool> {
    let roots = match project.to_str() {
        Some(".") => {
            Roots::Directory(fs::canonicalize(".").wrap_err("Could not open the current folder")?)
        }
        Some("all") => Roots::System,
        _ if explicit_path(project) => Roots::Projects(vec![workspace_for_path(project)?]),
        _ => {
            let listing = listing()?;
            let path = project_path(ui, &listing, project)?;
            return clean_inventory(ui, &cargo()?, listing, Some(&path), None, dry_run, None);
        }
    };
    clean_roots(ui, &roots, dry_run, None)
}

pub fn clean_roots(ui: &Ui, roots: &Roots, dry_run: bool, store: Option<&Store>) -> Result<bool> {
    let cargo = cargo()?;
    let listing = inventory(&cargo, roots)?;
    let strict = !matches!(roots, Roots::System);
    let directory = match roots {
        Roots::Directory(path) => Some(path.as_path()),
        _ => None,
    };
    let incomplete = strict && listing.unreadable_entries > 0;
    clean_inventory(ui, &cargo, listing, None, directory, dry_run, store)
        .map(|ok| ok && !incomplete)
}

fn clean_inventory(
    ui: &Ui,
    cargo: &Cargo,
    inventory: Inventory,
    project: Option<&Path>,
    directory: Option<&Path>,
    dry_run: bool,
    store: Option<&Store>,
) -> Result<bool> {
    #[derive(Serialize)]
    struct Outcome<'a> {
        path: &'a Path,
        bytes: u64,
        status: &'static str,
        issue: Option<String>,
    }
    #[derive(Serialize)]
    struct Summary<'a> {
        dry_run: bool,
        total_bytes: u64,
        unreadable_entries: usize,
        unreadable_manifests: usize,
        caches: Vec<Outcome<'a>>,
    }

    let selected: BTreeSet<_> = inventory
        .projects
        .iter()
        .filter(|item| project.is_none_or(|path| item.path == path))
        .flat_map(|project| &project.caches)
        .collect();
    let mut summary = Summary {
        dry_run,
        total_bytes: 0,
        unreadable_entries: inventory.unreadable_entries,
        unreadable_manifests: inventory.unreadable_manifests,
        caches: Vec::new(),
    };
    let local_store = (!dry_run && store.is_none())
        .then(Store::open)
        .transpose()?;
    let store = store.or(local_store.as_ref());
    let _lock = store.map(|store| store.lock("cleanup.lock")).transpose()?;
    let mut success = true;
    let mut cleaner = None;
    for cache in inventory
        .caches
        .iter()
        .filter(|cache| selected.contains(&cache.path))
    {
        let (status, issue) = if let Some(issue) = &cache.issue {
            ("skipped", Some(issue.clone()))
        } else if !cache.cleanable() {
            continue;
        } else if directory.is_some_and(|root| !cache.path.starts_with(root)) {
            (
                "skipped",
                Some("Cache is outside the current folder".to_owned()),
            )
        } else if dry_run {
            ("would_clean", None)
        } else {
            if cleaner.is_none() {
                cleaner = Some(
                    Cleaner::new(cargo, &store.expect("cleanup uses a state folder").path)
                        .map_err(|error| eyre!("{error:#}"))?,
                );
            }
            match cleaner
                .as_ref()
                .expect("cleaner was initialized")
                .clean(cache)
            {
                Ok(true) => ("cleaned", None),
                Ok(false) => continue,
                Err(error) => ("failed", Some(error.to_string())),
            }
        };
        if let Some(issue) = &issue {
            ui.warn(&format!("{} ({})", ui::path(&cache.path), ui::safe(issue)));
            success = false;
        } else {
            summary.total_bytes = summary.total_bytes.saturating_add(cache.bytes);
            if !ui.json {
                let verb = if dry_run { "Would clean" } else { "Cleaned" };
                println!(
                    "{}  {}  {}",
                    console::style(verb).green(),
                    ui::size(cache.bytes),
                    ui::path(&cache.path)
                );
            }
        }
        summary.caches.push(Outcome {
            path: &cache.path,
            bytes: cache.bytes,
            status,
            issue,
        });
    }
    if ui.json {
        ui.json(&summary)?;
    } else {
        report_skips(ui, &inventory);
        if summary.caches.is_empty() {
            println!("No Cargo build caches to clean.");
        } else if dry_run {
            println!("{} could be freed.", ui::size(summary.total_bytes));
        } else {
            println!("{} of build files cleaned.", ui::size(summary.total_bytes));
        }
    }
    Ok(success && (inventory.unreadable_manifests == 0 || project.is_some()))
}

fn report_skips(ui: &Ui, listing: &Inventory) {
    if listing.unreadable_entries > 0 || listing.unreadable_manifests > 0 {
        ui.warn(&format!(
            "{} filesystem entries and {} Cargo manifests that could not be read.",
            listing.unreadable_entries, listing.unreadable_manifests
        ));
    }
}
