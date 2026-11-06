use crate::Cargo;
use anyhow::{Context, Result, ensure};
use rustix::fs::{Mode, OFlags, open};
use serde::Serialize;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

const SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

#[derive(Debug, Serialize)]
pub struct Cache {
    pub path: PathBuf,
    pub bytes: u64,
    pub issue: Option<String>,
    #[serde(skip)]
    identity: Option<(u64, u64)>,
}

impl Cache {
    pub fn cleanable(&self) -> bool {
        self.identity.is_some() && self.issue.is_none()
    }
}

pub(crate) struct Mounts {
    paths: Vec<PathBuf>,
}

impl Mounts {
    #[cfg(target_os = "linux")]
    pub(crate) fn read() -> Result<Self> {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        // Linux bind mounts can share the device ID of their parent.
        let table =
            fs::read("/proc/self/mountinfo").context("Could not check mounted filesystems")?;
        let paths = table
            .split(|b| *b == b'\n')
            .filter_map(|line| {
                let mut field = line.split(|b| *b == b' ').nth(4)?;
                let mut path = Vec::new();
                while let Some((&byte, rest)) = field.split_first() {
                    if byte == b'\\'
                        && rest.len() >= 3
                        && rest[..3].iter().all(|b| (b'0'..=b'7').contains(b))
                    {
                        path.push((rest[0] - b'0') * 64 + (rest[1] - b'0') * 8 + (rest[2] - b'0'));
                        field = &rest[3..];
                    } else {
                        path.push(byte);
                        field = rest;
                    }
                }
                Some(PathBuf::from(OsString::from_vec(path)))
            })
            .collect();
        Ok(Self { paths })
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn read() -> Result<Self> {
        Ok(Self { paths: Vec::new() })
    }

    fn contains_mount(&self, path: &Path) -> bool {
        self.paths.iter().any(|mount| mount.starts_with(path))
    }
}

pub(crate) fn marked(path: &Path) -> bool {
    let tag = path.join("CACHEDIR.TAG");
    let Ok(fd) = open(
        &tag,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    let mut tag = File::from(fd);
    let uid = rustix::process::geteuid().as_raw();
    if !tag
        .metadata()
        .is_ok_and(|meta| meta.is_file() && meta.uid() == uid)
    {
        return false;
    }
    let mut signature = [0; SIGNATURE.len()];
    tag.read_exact(&mut signature).is_ok() && signature == SIGNATURE
}

fn identity(path: &Path, mounts: &Mounts) -> Result<Option<(u64, u64)>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Could not read the cache folder"),
    };
    let uid = rustix::process::geteuid().as_raw();
    ensure!(
        meta.is_dir() && meta.uid() == uid,
        "Cache is a link or belongs to another user"
    );
    ensure!(
        path.canonicalize()? == path,
        "Cache path goes through a link"
    );
    let parent = path
        .parent()
        .context("Cache points at the filesystem root")?;
    ensure!(
        fs::metadata(parent)?.dev() == meta.dev() && !mounts.contains_mount(path),
        "Cache contains a mounted filesystem"
    );
    ensure!(marked(path), "Cache has no valid Cargo cache marker");

    // ASVS 5.3.2: reject paths another user could swap before Cargo opens them.
    for ancestor in path.ancestors() {
        let directory = fs::symlink_metadata(ancestor)?;
        ensure!(
            directory.is_dir()
                && (directory.uid() == uid || directory.uid() == 0)
                && (directory.mode() & 0o022 == 0 || directory.mode() & 0o1000 != 0),
            "Cache path is writable by other users"
        );
    }
    ensure!(
        ["Cargo.toml", ".git", ".hg", ".svn"].iter().all(|name| {
            fs::symlink_metadata(path.join(name))
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
        }),
        "Cache contains a project or repository"
    );
    Ok(Some((meta.dev(), meta.ino())))
}

pub(crate) fn inspect(path: &Path, mounts: &Mounts, overlaps_project: bool) -> Cache {
    let mut cache = Cache {
        path: path.to_path_buf(),
        bytes: 0,
        issue: None,
        identity: None,
    };
    let checked = (|| -> Result<()> {
        ensure!(
            !overlaps_project,
            "Cache path contains a Cargo project or another cache"
        );
        let Some(id) = identity(path, mounts)? else {
            return Ok(());
        };
        cache.identity = Some(id);
        let mut hardlinks = HashSet::new();
        let uid = rustix::process::geteuid().as_raw();
        for item in WalkDir::new(path)
            .follow_links(false)
            .follow_root_links(false)
        {
            let item = item.context("Could not read all files in the cache")?;
            let meta = item
                .metadata()
                .context("Could not read all files in the cache")?;
            ensure!(meta.dev() == id.0, "Cache contains a mounted filesystem");
            ensure!(
                item.depth() == 0
                    || !["Cargo.toml", ".git", ".hg", ".svn"]
                        .iter()
                        .any(|name| item.file_name() == *name),
                "Cache contains a project or repository"
            );
            if meta.is_dir() {
                ensure!(
                    meta.uid() == uid && meta.mode() & 0o022 == 0,
                    "Cache contains a folder writable by other users"
                );
            }
            if meta.is_file() && (meta.nlink() == 1 || hardlinks.insert((meta.dev(), meta.ino()))) {
                cache.bytes = cache
                    .bytes
                    .saturating_add(meta.blocks().saturating_mul(512));
            }
        }
        Ok(())
    })();
    cache.issue = checked.err().map(|error| error.to_string());
    cache
}

pub struct Cleaner<'a> {
    cargo: &'a Cargo,
    sandbox: tempfile::TempDir,
    mounts: Mounts,
}

impl<'a> Cleaner<'a> {
    pub fn new(cargo: &'a Cargo, parent: &Path) -> Result<Self> {
        let sandbox =
            tempfile::tempdir_in(parent).context("Could not create a temporary Cargo workspace")?;
        fs::write(
            sandbox.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )?;
        Ok(Self {
            cargo,
            sandbox,
            mounts: Mounts::read()?,
        })
    }

    pub fn clean(&self, cache: &Cache) -> Result<bool> {
        ensure!(cache.cleanable(), "Cache did not pass validation");
        ensure!(
            !self.sandbox.path().starts_with(&cache.path),
            "Cache contains cargo-cleaner state"
        );
        let Some(current) = identity(&cache.path, &self.mounts)? else {
            return Ok(false);
        };
        ensure!(
            Some(current) == cache.identity,
            "Cache changed since the scan. Run the command again."
        );
        self.cargo.clean_cache(self.sandbox.path(), &cache.path)?;
        Ok(true)
    }
}
