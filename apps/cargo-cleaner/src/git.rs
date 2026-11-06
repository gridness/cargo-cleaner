use eyre::{Result, WrapErr, ensure};
use std::{
    ffi::OsString,
    io,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn git() -> Command {
    let mut command = Command::new("git");
    command.env("LC_ALL", "C");
    command
}

pub fn version() -> Result<String> {
    let output = git()
        .arg("--version")
        .output()
        .wrap_err("Could not run Git")?;
    ensure!(output.status.success(), "Could not get the Git version");
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_path(output: &Output) -> Result<PathBuf> {
    let bytes = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
    let path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
    ensure!(path.is_absolute(), "Git returned a non-absolute path");
    Ok(path)
}

pub fn root() -> Result<Option<PathBuf>> {
    let output = match git().args(["rev-parse", "--show-toplevel"]).output() {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).wrap_err("Could not run Git"),
    };
    if output.status.success() {
        return Ok(Some(git_path(&output)?));
    }
    let error = String::from_utf8_lossy(&output.stderr);
    if error.starts_with("fatal: not a git repository")
        || error.starts_with("fatal: this operation must be run in a work tree")
    {
        return Ok(None);
    }
    eyre::bail!("Could not find the Git worktree: {}", error.trim());
}

pub fn common_dir(root: &Path) -> Result<PathBuf> {
    rev_parse(root, &["--git-common-dir"])
}

pub fn hooks_dir(root: &Path) -> Result<PathBuf> {
    rev_parse(root, &["--git-path", "hooks"])
}

fn rev_parse(root: &Path, args: &[&str]) -> Result<PathBuf> {
    let output = git()
        .current_dir(root)
        .args(["rev-parse", "--path-format=absolute"])
        .args(args)
        .output()
        .wrap_err("Could not run Git")?;
    ensure!(
        output.status.success(),
        "Could not locate Git's hooks directory: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    git_path(&output)
}
