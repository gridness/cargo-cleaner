use super::Job;
use eyre::{Result, WrapErr, bail, ensure};
use rustix::fs::{Mode, OFlags, open};
use std::{
    fs::{self, DirBuilder, File, TryLockError},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};

pub struct Store {
    pub path: PathBuf,
}

impl Store {
    pub fn open() -> Result<Self> {
        let path = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .ok_or_else(|| eyre::eyre!("Could not find your user state folder"))?
            .join("cargo-cleaner");
        Self::at(&path)
    }

    pub(super) fn at(path: &Path) -> Result<Self> {
        ensure!(
            path.is_absolute(),
            "The state folder must be an absolute path"
        );
        let parent = path
            .parent()
            .ok_or_else(|| eyre::eyre!("The state folder needs a parent"))?;
        fs::create_dir_all(parent).wrap_err("Could not create the user state folder")?;
        match DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).wrap_err("Could not create the cargo-cleaner state folder");
            }
        }
        let meta = fs::symlink_metadata(path)?;
        ensure!(
            meta.is_dir()
                && meta.uid() == rustix::process::geteuid().as_raw()
                && meta.mode() & 0o077 == 0,
            "The cargo-cleaner state folder must be private and owned by you"
        );
        Ok(Self {
            path: path.canonicalize()?,
        })
    }

    pub fn lock(&self, name: &str) -> Result<File> {
        // ASVS 2.3.4: serialize scheduler updates and prevent concurrent cleans.
        let fd = open(
            self.path.join(name),
            OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .wrap_err("Could not open the cargo-cleaner lock")?;
        let file = File::from(fd);
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(TryLockError::WouldBlock) => {
                bail!("Another cargo-cleaner command is running. Try again shortly.")
            }
            Err(TryLockError::Error(error)) => {
                Err(error).wrap_err("Could not lock cargo-cleaner state")
            }
        }
    }

    pub(super) fn jobs(&self) -> Result<Vec<Job>> {
        let mut jobs = Vec::new();
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") && entry.file_type()?.is_file() {
                let file = File::open(&path)?;
                let job: Job =
                    serde_json::from_reader(file).wrap_err("Could not read a saved schedule")?;
                ensure!(
                    valid_id(&job.id)
                        && path.file_stem().is_some_and(|stem| stem == job.id.as_str()),
                    "A saved schedule has an invalid ID"
                );
                jobs.push(job);
            }
        }
        jobs.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        Ok(jobs)
    }

    pub(super) fn save(&self, mut job: Job) -> Result<Job> {
        let mut temp = tempfile::Builder::new()
            .prefix("job-")
            .tempfile_in(&self.path)?;
        job.id = temp
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| eyre::eyre!("Could not create a schedule ID"))?
            .to_owned();
        ensure!(valid_id(&job.id), "Could not create a schedule ID");
        serde_json::to_writer(temp.as_file_mut(), &job)?;
        temp.as_file().sync_all()?;
        temp.persist_noclobber(self.path.join(format!("{}.json", job.id)))
            .wrap_err("Could not save the schedule")?;
        Ok(job)
    }

    pub(super) fn remove(&self, id: &str) -> Result<()> {
        ensure!(valid_id(id), "Invalid schedule ID");
        fs::remove_file(self.path.join(format!("{id}.json")))
            .wrap_err("Could not remove the schedule")
    }
}

fn valid_id(id: &str) -> bool {
    id.strip_prefix("job-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 32
            && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}
