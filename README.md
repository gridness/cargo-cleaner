# cargo-cleaner

Find Cargo workspaces, see how much disk space their build caches take, and clean them in one command. Runs on macOS and Linux with Cargo installed.

## Install

```sh
brew install gridness/oosama/cargo-cleaner
```

To build from this repository, run `cargo install --path apps/cargo-cleaner --locked`. Homebrew installs shell completions. For a source install, `cargo-cleaner complete --shell zsh` prints a completion script. Replace `zsh` with `bash`, `fish`, `powershell`, or `elvish` as needed.

## Scan and clean

```sh
cargo-cleaner scan
cargo-cleaner scan --path ~/dev --json
cargo-cleaner clean . --dry-run
cargo-cleaner clean .
cargo-cleaner clean all
cargo-cleaner clean my-project
cargo-cleaner clean ./my-project
```

`scan` searches readable system directories and lists workspaces by folder name, with the largest first. It skips system directories and common dependency caches. Each workspace gets one row. The total counts shared cache paths once, and sizes reflect allocated disk blocks.

`clean .` searches the current folder and its subfolders, with cleanup confined to that tree. A project name selects a workspace. If names collide, use the interactive picker or pass a path. Use `./project` for a relative path. `clean all` searches the same locations as `scan`. Use `--dry-run` to see the cleanup plan before running it.

Cleanup uses Cargo's reported target and build directories and requires a valid cache marker and safe permissions. It skips symlinked cache paths, mounts, and cache paths that contain a discovered workspace or another cache. A shared cache is cleaned as a unit. Use `--json` for machine-readable results. Cleanup returns a nonzero exit status if a selected cache fails or is skipped for safety.

## Schedule

```sh
cargo-cleaner schedule create "daily at 02:30" --all
cargo-cleaner schedule create every Sunday at 02:30 --project my-project --project ./another
cargo-cleaner schedule create
cargo-cleaner schedule list --json
cargo-cleaner schedule stop --id JOB_ID
cargo-cleaner schedule remove --all
```

Use `HH:MM` for a daily run, `daily at HH:MM`, `every <weekday> at HH:MM`, or `hourly`. Times follow the user crontab. The machine needs a running cron service and must be awake at the scheduled time.

`--all` scans the system at each run. `--project` saves the chosen workspace paths. With a terminal, omitting both opens the project picker. Use the arrow keys to move and Space to toggle projects. For scripts, pass the time and scope explicitly.

`stop` disables future runs and keeps the schedule. `remove` also deletes the saved entry. With one schedule, you can omit its ID. A cleanup already in progress can finish.

## Releases

The release workflow builds on `main`, bumps the patch version, and publishes a tag with GitHub-generated notes and archives for macOS and Linux on Intel and ARM. Set a higher base version in the workspace manifest to start a new version line. The Homebrew tap checks published releases and updates its formula using the archive checksums from GitHub.
