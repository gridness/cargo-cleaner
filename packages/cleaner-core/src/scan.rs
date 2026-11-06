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
pub struct Inventory {
    pub projects: Vec<Project>,
    pub caches: Vec<Cache>,
    pub unreadable_entries: usize,
    pub unreadable_manifests: usize,
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
                    Err(_) => unreadable_entries += 1,
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
    for report in reports {
        let Ok(report) = report else {
            unreadable_manifests += 1;
            continue;
        };
        let Ok(workspace) = report.workspace_root.canonicalize() else {
            unreadable_manifests += 1;
            continue;
        };
        if directory
            .as_ref()
            .is_some_and(|root| !workspace.starts_with(root))
        {
            continue;
        }
        if matches!(scope, Roots::Projects(paths) if !paths.contains(&workspace)) {
            unreadable_manifests += 1;
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
