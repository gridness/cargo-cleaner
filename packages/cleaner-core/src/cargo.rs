use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    env, fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub struct Cargo(PathBuf);

#[derive(Deserialize)]
pub(crate) struct Metadata {
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    #[serde(default)]
    pub build_directory: Option<PathBuf>,
}

impl Cargo {
    pub fn find() -> Result<Self> {
        let fallback = dirs::home_dir().map(|home| home.join(".cargo/bin"));
        let path = env::var_os("PATH").unwrap_or_default();
        let cargo = env::split_paths(&path)
            .filter(|path| path.is_absolute())
            .chain(fallback)
            .chain([
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ])
            .map(|dir| dir.join("cargo"))
            .find(|path| {
                fs::metadata(path)
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            })
            .context("Cargo is missing. Install Rust or add Cargo to PATH.")?;
        let rustup = cargo.with_file_name("rustup");
        let is_proxy = fs::metadata(&cargo)
            .ok()
            .zip(fs::metadata(&rustup).ok())
            .is_some_and(|(a, b)| (a.dev(), a.ino()) == (b.dev(), b.ino()));
        if !is_proxy {
            return Ok(Self(cargo.canonicalize()?));
        }

        // Resolve the proxy outside discovered projects. A project's rust-toolchain
        // file can otherwise select a local executable or trigger a download.
        let result = Command::new(rustup)
            .args(["which", "cargo"])
            .current_dir("/")
            .stdin(Stdio::null())
            .output()
            .context("Could not find the Cargo executable through rustup")?;
        ensure!(
            result.status.success(),
            "Rustup needs an installed default toolchain"
        );
        let executable = PathBuf::from(std::str::from_utf8(&result.stdout)?.trim());
        ensure!(
            executable.is_absolute(),
            "Rustup returned an invalid Cargo path"
        );
        Ok(Self(executable))
    }

    pub fn version(&self) -> Result<String> {
        let output = Command::new(&self.0)
            .arg("--version")
            .current_dir("/")
            .stdin(Stdio::null())
            .output()
            .context("Could not start Cargo")?;
        ensure!(output.status.success(), "Cargo --version failed");
        Ok(std::str::from_utf8(&output.stdout)?.trim().to_owned())
    }

    pub fn workspace(&self, folder: &Path) -> Result<PathBuf> {
        let folder = folder
            .canonicalize()
            .context("Could not open the project folder")?;
        self.metadata(&folder.join("Cargo.toml"))?
            .workspace_root
            .canonicalize()
            .context("Could not open the Cargo workspace")
    }

    pub(crate) fn metadata(&self, manifest: &Path) -> Result<Metadata> {
        let info =
            fs::symlink_metadata(manifest).context("No readable Cargo.toml in this folder")?;
        ensure!(
            info.is_file()
                && info.uid() == rustix::process::geteuid().as_raw()
                && info.len() <= 8 * 1024 * 1024,
            "Cargo.toml must be a regular file owned by you and at most 8 MiB"
        );
        // ASVS 1.2.5: pass paths as arguments and use the built-in metadata
        // command offline, without resolving dependencies or running build scripts.
        let output = Command::new(&self.0)
            .args([
                "--offline",
                "metadata",
                "--format-version",
                "1",
                "--no-deps",
                "--manifest-path",
            ])
            .arg(manifest)
            .current_dir(manifest.parent().context("Cargo manifest has no folder")?)
            .stdin(Stdio::null())
            .output()
            .context("Could not start Cargo")?;
        ensure!(
            output.status.success(),
            "Cargo could not read the manifest or its configuration"
        );
        serde_json::from_slice(&output.stdout).context("Cargo returned invalid project information")
    }

    pub(crate) fn clean_cache(&self, sandbox: &Path, path: &Path) -> Result<()> {
        // ASVS 15.2.5: use an empty workspace and Cargo home. Project config and
        // inherited Cargo variables must never redirect this deletion.
        let output = Command::new(&self.0)
            .env_clear()
            .env("CARGO_HOME", sandbox.join("home"))
            .env("RUSTC", self.0.with_file_name("rustc"))
            .env("RUSTC_WRAPPER", "")
            .env("RUSTC_WORKSPACE_WRAPPER", "")
            .current_dir(sandbox)
            .args([
                "--offline",
                "--config",
                "build.build-dir='target'",
                "clean",
                "--target-dir",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .context("Could not start Cargo")?;
        ensure!(output.status.success(), "Cargo could not clean this cache");
        Ok(())
    }
}
