# Working in this repository

- Use the root Cargo workspace for shared dependencies and package metadata. Put executable entry points in `apps/` and reusable Rust code in `packages/`. Follow the [Modrinth workspace](https://github.com/modrinth/code/blob/main/Cargo.toml) for this split.
- Keep argument parsing, terminal output, and scheduler integration in `apps/cargo-cleaner`. Keep project discovery and cache safety in `packages/cleaner-core` so scheduled and manual cleanup share the same checks.
- Use long CLI flags. Keep JSON output on stdout and diagnostics on stderr. Escape terminal control characters in paths and other external text. Use `anyhow` in the core and `eyre` at the CLI boundary.
- Require a verified Cargo cache before cleanup. Preserve the protections against symlinks, mounts, and paths containing project sources. Run cleanup regression tests against temporary projects.
- Before landing changes, run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo test --workspace --locked`.
