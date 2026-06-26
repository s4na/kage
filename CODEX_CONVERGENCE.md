# CODEX Convergence Checklist

## Cycle 1

- Current hypothesis: `kage-rofs` has a lazy `GitTreeView` model but no actual read-only filesystem mount, so `--lower git-rofs` cannot become an overlayfs lowerdir.
- Failing gates:
  - actual rofs mount is missing;
  - `KAGE_TEST_ROFS=1` cannot prove a real mount;
  - rofs + overlay strict integration cannot run because rofs lower mount path does not exist;
  - CLI rejects `--lower git-rofs` instead of attempting a rofs mount.
- Files likely involved:
  - `crates/kage-rofs/Cargo.toml`
  - `crates/kage-rofs/src/lib.rs`
  - `crates/kage-cli/src/main.rs`
  - `crates/kage-cli/tests/cli.rs`
  - `crates/kage-git/tests/backend_trees.rs`
  - `README.md`
  - `docs/architecture.md`
  - `scripts/verify-rofs.sh`
  - `scripts/verify-rofs-overlay.sh`
- Planned fixes:
  - implement a narrow read-only FUSE filesystem around `GitTreeView`;
  - add a mount handle with idempotent unmount;
  - add strict `KAGE_TEST_ROFS=1` mount tests and allow-skip warning path;
  - integrate `--lower git-rofs` with `overlayfs` by mounting rofs at workspace `lower/` and passing it as overlay lowerdir;
  - keep fallback/exported as default;
  - improve diagnostics/docs.
- Commands to run:
  - focused `cargo test -p kage-rofs --all-features`;
  - focused CLI/gated tests;
  - full fmt/clippy/test/diff checks;
  - strict and allow-skip gated tests;
  - forbidden shortcut search.
- Current blocker classification: IMPLEMENTATION_DEFECT.

## Initial gap list

1. `lower/` is currently materialized by `GitRepo::export_tree` in `kage-git`, used by `kage-cli workspace create --lower exported` and fallback/overlay tests.
2. `git archive` remains in `GitRepo::export_tree`; this is acceptable only for fallback/debug/test exported lower.
3. The lazy lower provider should plug into `kage-cli workspace create` where lower is prepared before backend mount; for `overlayfs --lower git-rofs`, `lower/` should become a rofs mountpoint.
4. `WorkspaceSpec` already records `lower_kind`, but it does not record rofs mount lifecycle state beyond paths.
5. CLI already has `--lower`, but `git-rofs` currently errors instead of mounting.
6. Existing fallback tests prove exported lower and directory-merge behavior; `GitTreeView` pure tests prove model behavior only.
7. Comparison tests should additionally compare rofs mount behavior when `KAGE_TEST_ROFS=1` is available.
8. Environment-dependent tests needing gates: FUSE rofs mount (`KAGE_TEST_ROFS`), overlayfs (`KAGE_TEST_OVERLAY`), combined rofs+overlay, and trusted overlay xattrs.

## Cycle 2

- Current hypothesis: manual FUSE mount code can be compiled and default-tested, but this container lacks `/dev/fuse` and overlay mount capability.
- Failing gates:
  - `KAGE_TEST_ROFS=1` fails because `/dev/fuse` is unavailable;
  - `KAGE_TEST_OVERLAY=1` fails because overlay mount returns permission denied;
  - combined rofs+overlay strict test fails for both reasons.
- Classification: ENVIRONMENT_BLOCKER.
- Fix applied:
  - added narrow read-only FUSE protocol implementation around `GitTreeView`;
  - added `RofsMount` lifecycle and strict mount test body that reads files and verifies write failure when a mount succeeds;
  - added CLI rofs daemon path for `--backend overlayfs --lower git-rofs` without falling back to exported lower;
  - added gated rofs+overlay comparison test;
  - improved verification scripts and README/docs.
- Commands rerun:
  - `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace --all-features && git diff --check` passed.
  - `KAGE_TEST_ROFS=1 cargo test --workspace --all-features -- --nocapture` failed on `/dev/fuse is unavailable`.
  - `KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture` failed on overlay mount permission denied.
  - allow-skip variants passed with explicit warnings.
- Current blocker classification: ENVIRONMENT_BLOCKER.
