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

## Cycle 3

- Failing gate: FUSE protocol behavior was not sufficiently proven without a kernel mount.
- Classification: TEST_DEFECT.
- Files inspected:
  - `crates/kage-rofs/src/lib.rs`
- Fix applied:
  - added mount-free protocol tests for stable inode lookup, root inode, `FUSE_INIT`, `FUSE_FORGET`, getattr mode/size, full and offset reads, EOF reads, binary reads, exact readlink data, root and nested readdir, readdir offset continuation, Unicode/spaced names, missing lookup errno, read-only mutation errno, large-file partial reads, and large-directory readdir;
  - corrected test ABI offsets for `fuse_attr.mode` and `fuse_attr.size` during the cycle.
- Tests run:
  - `cargo test -p kage-rofs --all-features` initially failed on incorrect test-side attr offsets;
  - after fixing the offsets, `cargo test -p kage-rofs --all-features` passed.
- Result: mount-free FUSE request/response behavior is now covered for the narrow read-only operations kage-rofs implements.
- Remaining risk: kernel FUSE mount execution still requires `/dev/fuse`; the implementation remains hand-written and must be treated as higher risk until strict mount tests pass in a capable environment.

## Cycle 4

- Failing gate: rofs daemon lifecycle and CLI rollback behavior were under-tested without kernel mounts.
- Classification: TEST_DEFECT.
- Files inspected:
  - `crates/kage-cli/src/main.rs`
  - `crates/kage-cli/tests/cli.rs`
- Fix applied:
  - extracted `rofs_serve_command` so argument construction can be tested without shell interpolation;
  - added a stale-pid cleanup/idempotency test for `stop_rofs_daemon`;
  - added CLI integration tests proving invalid `rofs-serve` input fails clearly and `--backend overlayfs --lower git-rofs` does not record a workspace or silently export `lower/` when rofs startup fails.
- Tests run:
  - `cargo test -p kage-cli --all-features` passed.
- Result: non-kernel CLI lifecycle and no-silent-fallback behavior are covered.
- Remaining risk: real rofs-starts-then-overlay-fails rollback is still verified only by strict gated integration in a capable kernel environment.

## Cycle 5

- Failing gate: metadata migration and reviewer verification route were incomplete.
- Classification: DOCUMENTATION_DEFECT.
- Files inspected:
  - `crates/kage-core/src/lib.rs`
  - `README.md`
  - `scripts/verify-rofs.sh`
  - `scripts/verify-overlayfs.sh`
  - `scripts/verify-rofs-overlay.sh`
- Fix applied:
  - added an old-workspace metadata test proving missing `lower_kind` defaults to `exported`;
  - added README verification levels and explicitly stated that allow-skip is not proof;
  - added `scripts/run-privileged-linux-tests.sh` as a concrete privileged Docker verification route for strict rofs/overlay/combined tests.
- Tests run:
  - `cargo test -p kage-core --lib` passed;
  - the full default command chain passed.
- Result: metadata compatibility and reviewer-facing verification instructions are improved.
- Remaining risk: the privileged route requires a host with Docker, `/dev/fuse`, and mount capability; this container does not provide those kernel facilities.

## Cycle 6

- Failing gate: strict kernel mount tests still fail in the current container.
- Classification: ENVIRONMENT_BLOCKER.
- Files inspected:
  - `crates/kage-rofs/src/lib.rs`
  - `crates/kage-overlay/src/lib.rs`
  - `scripts/verify-rofs.sh`
  - `scripts/verify-rofs-overlay.sh`
- Fix applied:
  - narrowed strict-test failures to the kernel boundary after mount-free protocol/lifecycle/default tests passed;
  - reran strict and allow-skip test variants;
  - ran diagnostics proving `/dev/fuse` is unavailable and overlay mount returns permission denied.
- Tests run:
  - `KAGE_TEST_ROFS=1 cargo test --workspace --all-features -- --nocapture` failed only at the strict rofs mount body with `/dev/fuse is unavailable`;
  - `KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture` failed only at strict overlay mount with permission denied;
  - `KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture` failed for the same kernel-boundary reasons;
  - allow-skip variants passed with explicit warnings.
- Result: non-kernel gates are green; kernel mount verification remains blocked by the current environment.
- Remaining risk: strict rofs, overlayfs, and combined rofs+overlay+commit-back behavior must be run on a privileged Linux host/VM before production claims.


## Cycle 7

- Failing gate: installable dependency status and maintained FUSE crate replacement had not been re-attempted after the stronger review request.
- Classification: ENVIRONMENT_BLOCKER for package/crate fetch, DESIGN_DEFECT for undocumented hand-written FUSE risk.
- Files inspected:
  - `crates/kage-rofs/src/lib.rs`
  - `docs/fuse-risk.md`
- Fix applied:
  - attempted `apt-get update && apt-get install -y fuse3 libfuse3-dev pkg-config build-essential attr util-linux fuse-overlayfs findmnt`; the proxy returned HTTP 403 for Ubuntu repositories;
  - attempted `cargo search fuser --limit 1`; crates.io index fetch failed with proxy HTTP 403;
  - kept the hand-written FUSE boundary and documented the risk, supported opcodes, unsupported opcodes, unsafe ABI areas, mount-free tests, and strict kernel proof requirements in `docs/fuse-risk.md`.
- Tests run:
  - `cargo test -p kage-cli --all-features` passed after follow-up lifecycle test additions.
- Result: dependency setup/fuser replacement is blocked by repository/network access, not by missing code intent.
- Remaining risk: hand-written FUSE remains a review risk until fuser replacement or strict kernel validation.

## Cycle 8

- Failing gate: no single strict runtime smoke test covered CLI create -> rofs lower -> overlayfs -> edit -> commit -> discard.
- Classification: TEST_DEFECT.
- Files inspected:
  - `crates/kage-cli/tests/cli.rs`
  - `crates/kage-cli/src/main.rs`
- Fix applied:
  - added a strict `KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1` CLI smoke test that creates a real workspace, reads through merged, modifies/adds/deletes files, commits, verifies the Git tree, and discards;
  - added mount-free fake lifecycle tests for rofs-start/overlay-fail rollback, metadata write on success, failed commit preserving mount state, ref-not-updated on failed commit, and discard ordering/idempotency.
- Tests run:
  - `cargo test -p kage-cli --all-features` passed.
- Result: runtime smoke coverage exists and is strict-gated; non-kernel lifecycle ordering is tested.
- Remaining risk: the strict smoke test still requires a capable kernel environment to execute its full body.

## Cycle 9

- Failing gate: reviewer runtime verification route needed a concrete named privileged container route and README current proof-level language.
- Classification: DOCUMENTATION_DEFECT.
- Files inspected:
  - `README.md`
  - `scripts/run-docker-privileged-rofs-overlay-tests.sh`
  - `scripts/run-privileged-linux-tests.sh`
- Fix applied:
  - added `scripts/run-docker-privileged-rofs-overlay-tests.sh`, which runs a privileged Docker container, passes `/dev/fuse`, attempts package installation, and runs default, strict rofs, strict overlay, combined, and runtime smoke tests;
  - made `scripts/run-privileged-linux-tests.sh` delegate to the named Docker route;
  - updated README with a Current proof level section stating Level 0 and Level 1 are proven here, Level 2-4 require a capable Linux host/VM, allow-skip is not proof, production readiness requires Level 4, and hand-written FUSE remains a review risk.
- Tests run:
  - full default command chain and strict diagnostics are required before final report.
- Result: REVIEWABLE_ENVIRONMENT_BLOCKED can be reached if default tests pass and strict failures remain kernel-only.
- Remaining risk: Docker itself is unavailable in this container, so the privileged route is committed for reviewers rather than executed here.

## Cycle 10

- Failing gate: strict runtime smoke initially reached overlay mount with an unmounted plain `lower/`, which showed `rofs_mount_ready` was accepting an ordinary directory as ready.
- Classification: IMPLEMENTATION_DEFECT.
- Files inspected:
  - `crates/kage-cli/src/main.rs`
  - `crates/kage-cli/tests/cli.rs`
- Fix applied:
  - changed `rofs_mount_ready` to validate `/proc/self/mountinfo` rather than `read_dir`, including mountinfo path unescaping;
  - added a unit test proving a plain directory is not considered a rofs mount and mountinfo escapes are decoded;
  - reran the strict runtime smoke test, which now fails before overlay mount with the correct `/dev/fuse is unavailable` rofs startup error.
- Tests run:
  - `cargo test -p kage-cli --all-features` passed;
  - full default command chain passed;
  - `KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1 cargo test --workspace --all-features -- --nocapture` failed at the strict rofs mount boundary because `/dev/fuse` is unavailable.
- Result: a real implementation bug below the kernel boundary was fixed; runtime smoke failure is now correctly narrowed to rofs mount availability.
- Remaining risk: strict runtime smoke still needs a capable Linux host/VM.
