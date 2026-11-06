---
name: cargo-cleaner
description: Reclaim disk space from Cargo build caches.
---

1. Run `cargo-cleaner clean . --dry-run` from the requested folder. Use `all` only for a requested system-wide cleanup.
2. Review the plan. When the user has authorized deletion, rerun without `--dry-run` and report bytes reclaimed and any skipped caches.
