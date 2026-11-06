use eyre::{Result, WrapErr, ensure, eyre};
use rustix::fs::{Mode, OFlags, open};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
};

pub fn directory(root: &Path, relative: &Path, create: bool) -> Result<PathBuf> {
    let mut path = root.to_owned();
    for component in relative.components() {
        // ASVS 5.3.2: keep managed files below the selected directory.
        let Component::Normal(name) = component else {
            return Err(eyre!("Expected a path below the selected directory"));
        };
        path.push(name);
        if create {
            match fs::create_dir(&path) {
                Ok(()) => continue,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        match fs::symlink_metadata(&path) {
            Ok(meta) => ensure!(meta.is_dir(), "{} must be a real directory", path.display()),
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

pub struct Contents {
    pub bytes: Vec<u8>,
    pub mode: u32,
}

pub fn read(path: &Path, limit: u64) -> Result<Option<Contents>> {
    let fd = match open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("Could not open {}", path.display()));
        }
    };
    let file = File::from(fd);
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "{} must be a regular file", path.display());
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "{} exceeds the size limit",
        path.display()
    );
    Ok(Some(Contents {
        bytes,
        mode: meta.permissions().mode() & 0o777,
    }))
}

pub fn write(path: &Path, bytes: &[u8], mode: u32, previous: Option<&Contents>) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| eyre!("Expected a parent directory"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().set_permissions(fs::Permissions::from_mode(
        previous.map_or(mode, |file| file.mode),
    ))?;
    temp.as_file().sync_all()?;
    // ASVS 2.3.3: publish a complete file, and refuse to clobber new files.
    if let Some(previous) = previous {
        ensure!(
            read(path, previous.bytes.len() as u64)?.is_some_and(|current| current.bytes
                == previous.bytes
                && current.mode == previous.mode),
            "{} changed during the update",
            path.display()
        );
        temp.persist(path)?;
    } else {
        temp.persist_noclobber(path)?;
    }
    Ok(())
}
