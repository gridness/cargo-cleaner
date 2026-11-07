use crate::{
    Cache, Cargo,
    cache::{Mounts, inspect, marked},
};
use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};
use walkdir::{DirEntry, WalkDir};

pub enum Roots {
    System,
    Directory(PathBuf),
    Projects(Vec<PathBuf>),
}

#[derive(Debug, Serialize)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub caches: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct ScanIssue {
    pub path: Option<PathBuf>,
    pub reason: String,
    pub kind: &'static str,
}

fn record_issue(
    issues: &mut Vec<ScanIssue>,
    path: Option<&Path>,
    kind: &'static str,
    reason: String,
) {
    // Keep exact counters and sample both kinds, even if filesystem errors fill
    // their budget before Cargo reads any manifests.
    if issues.iter().filter(|issue| issue.kind == kind).count() < 100 {
        issues.push(ScanIssue {
            path: path.map(Path::to_owned),
            reason,
            kind,
        });
    }
}

#[derive(Debug, Serialize)]
pub struct Inventory {
    pub projects: Vec<Project>,
    pub caches: Vec<Cache>,
    pub unreadable_entries: usize,
    pub unreadable_manifests: usize,
    pub issues: Vec<ScanIssue>,
}

impl Inventory {
    pub fn bytes(&self) -> u64 {
        self.caches
            .iter()
            .fold(0, |total, cache| total.saturating_add(cache.bytes))
    }
}

pub fn scan(cargo: &Cargo, scope: &Roots) -> Result<Inventory> {
    let mut unreadable_entries = 0;
    let mut issues = Vec::new();
    let mut manifests = Vec::new();
    let directory = match scope {
        Roots::Directory(path) => Some(
            path.canonicalize()
                .context("Could not open the scan folder")?,
        ),
        _ => None,
    };
    match scope {
        Roots::Projects(paths) => {
            for path in paths {
                if path
                    .canonicalize()
                    .as_ref()
                    .is_ok_and(|actual| actual == path)
                {
                    manifests.push(path.join("Cargo.toml"));
                } else {
                    unreadable_entries += 1;
                    record_issue(
                        &mut issues,
                        Some(path),
                        "entry",
                        "The saved project folder is missing, inaccessible, or now goes through a link".to_owned(),
                    );
                }
            }
        }
        Roots::System | Roots::Directory(_) => {
            let root = directory.as_deref().unwrap_or(Path::new("/"));
            ensure!(root.is_dir(), "Choose a folder to scan");
            let system = matches!(scope, Roots::System);
            for item in WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| visit(entry, system))
            {
                match item {
                    Ok(entry)
                        if entry.file_name() == "Cargo.toml" && entry.file_type().is_file() =>
                    {
                        manifests.push(entry.into_path());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        unreadable_entries += 1;
                        record_issue(&mut issues, error.path(), "entry", error.to_string());
                    }
                }
            }
        }
    }

    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()?;
    let reports = pool.install(|| {
        manifests
            .par_iter()
            .map(|manifest| cargo.metadata(manifest))
            .collect::<Vec<_>>()
    });
    let mut unreadable_manifests = 0;
    let mut workspaces = BTreeMap::<PathBuf, BTreeSet<PathBuf>>::new();
    for (manifest, report) in manifests.iter().zip(reports) {
        let report = match report {
            Ok(report) => report,
            Err(error) => {
                unreadable_manifests += 1;
                record_issue(
                    &mut issues,
                    Some(manifest),
                    "manifest",
                    format!("{error:#}"),
                );
                continue;
            }
        };
        let workspace = match report.workspace_root.canonicalize() {
            Ok(workspace) => workspace,
            Err(error) => {
                unreadable_manifests += 1;
                record_issue(
                    &mut issues,
                    Some(manifest),
                    "manifest",
                    format!("Could not open the workspace folder: {error}"),
                );
                continue;
            }
        };
        if directory
            .as_ref()
            .is_some_and(|root| !workspace.starts_with(root))
        {
            continue;
        }
        if matches!(scope, Roots::Projects(paths) if !paths.contains(&workspace)) {
            unreadable_manifests += 1;
            record_issue(
                &mut issues,
                Some(manifest),
                "manifest",
                "This manifest belongs to a different workspace than the selected project"
                    .to_owned(),
            );
            continue;
        }
        let caches = workspaces.entry(workspace).or_default();
        caches.insert(report.target_directory);
        caches.extend(report.build_directory);
    }

    let paths = workspaces
        .values()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    let containing_paths: BTreeSet<_> = workspaces
        .keys()
        .flat_map(|path| path.ancestors())
        .chain(paths.iter().flat_map(|path| path.ancestors().skip(1)))
        .collect();
    let mounts = Mounts::read()?;
    let projects = workspaces
        .iter()
        .map(|(path, caches)| Project {
            name: path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned(),
            path: path.clone(),
            caches: caches.iter().cloned().collect(),
        })
        .collect();
    let mut caches: Vec<_> = pool.install(|| {
        paths
            .par_iter()
            .map(|path| inspect(path, &mounts, containing_paths.contains(path.as_path())))
            .collect()
    });
    caches.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(Inventory {
        projects,
        caches,
        unreadable_entries,
        unreadable_manifests,
        issues,
    })
}

fn visit(entry: &DirEntry, system: bool) -> bool {
    if !entry.file_type().is_dir() || entry.depth() == 0 {
        return true;
    }
    let path = entry.path();
    if system
        && ([
            "/dev",
            "/proc",
            "/sys",
            "/run",
            "/System",
            "/Library",
            "/private/var/db",
            "/private/var/vm",
            "/usr/share",
            "/usr/lib",
            "/var/lib/docker",
            "/var/lib/containers",
        ]
        .iter()
        .any(|skip| path == Path::new(skip)))
    {
        return false;
    }
    let name = entry.file_name();
    if [".git", ".hg", ".svn", "node_modules", ".Trash"]
        .iter()
        .any(|skip| name == *skip)
    {
        return false;
    }
    let parent = path.parent().and_then(Path::file_name);
    if (parent == Some(OsStr::new(".cargo")) && (name == "registry" || name == "git"))
        || (parent == Some(OsStr::new(".rustup")) && name == "toolchains")
    {
        return false;
    }
    // Only prune a directory named target when it is marked as a cache.
    name != "target" || !marked(path) || fs::symlink_metadata(path.join("Cargo.toml")).is_ok()
}
