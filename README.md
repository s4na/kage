# kage

A Git-native Copy-on-Write workspace runtime prototype written in Rust.

kage is intended to make short-lived, isolated workspaces from Git refs without using `git checkout` or `git worktree` as the primary workspace model. Git remains the durable source of truth. kage records workspace-specific edits in an upper layer, computes diffs against the parent Git tree, and commits back by constructing a synthetic Git tree and updating the requested ref explicitly.

## Problem

Large repositories are expensive to repeatedly materialize for agents, containers, CI sandboxes, and remote development sessions. Most workspaces modify only a small set of paths. kage aims to make each workspace contain only:

- workspace metadata;
- changed or added files in an upper layer;
- deletion metadata / whiteout-like state;
- disposable runtime/cache data.

## Architecture overview

Target model:

```text
Git Object Store
      ↓
Read-only Git Tree Snapshot / Model
      ↓
Existing CoW Overlay Backend
      ↓
Workspace Mount
      ↓
IDE / Container / AI Agent / Test Runner
```

Current implementation status:

- `kage-core`: workspace metadata, runtime paths, path validation, and the read-only `BaseLayer` trait boundary.
- `kage-git`: Git CLI backed object/ref adapter. Commit-back now applies the workspace upper layer to the parent tree through a temporary Git index instead of scanning a normal Git worktree.
- `kage-overlay`: backend trait plus two implemented backends: non-privileged `fallback` directory-merge for debug/tests, and opt-in `overlayfs` for native Linux overlay mounts.
- `kage-container`: container argv construction for Docker, Podman, and Apple Container.
- `kage-cli`: CLI for repository/workspace lifecycle, diff, commit, local exec, and container exec.

The fallback backend still exports a Git tree into an internal lower directory to provide a plain editable `merged` directory in unprivileged environments. That fallback is development/debug infrastructure. The first production-like backend boundary is `overlayfs`, which runs a native Linux overlay mount over lower/upper/work/merged directories when explicitly selected. The remaining production gap is replacing the exported lower directory with a lazy read-only Git tree view inside the managed Linux runtime.

## Apple Silicon / arm64 Linux v1 target

v1 targets Apple Silicon Macs as the host orchestration environment, with the primary filesystem/runtime substrate expected to be a managed arm64 Linux environment.

```text
Apple Silicon Mac host
  └─ kage Rust control plane
      ├─ workspace registry and metadata
      ├─ Git ref/tree/diff/commit-back orchestration
      └─ container/agent command orchestration

Managed arm64 Linux VM / container runtime
  ├─ Linux overlayfs or fuse-overlayfs backend (future production backend)
  ├─ workspace mount at /workspace
  └─ build/test/agent process execution
```

Native macOS filesystem mounting is **not** the primary path. It may be added later as an optional backend, but the intended production path is Linux-side filesystem semantics inside a managed VM/container boundary.

## Requirements

Current local development requirements:

- Rust 1.80+
- Git CLI
- POSIX-like shell utilities for the current fallback tree export path (`sh`, `tar`)
- Optional: Docker or Podman for `kage run`
- Optional: Apple Container CLI (`container`) for `kage run --apple-container`

No network access is required for the test suite.

## Build

```bash
cargo build --workspace
```

## Quickstart

Create a temporary repository for experimentation:

```bash
mkdir /tmp/kage-demo
cd /tmp/kage-demo
git init -b main
git config user.email kage@example.invalid
git config user.name "kage demo"
echo hello > README.md
git add README.md
git commit -m initial
```

Build kage and create a workspace:

```bash
cargo build --workspace
./target/debug/kage --home /tmp/kage-home workspace create --ref main --repo /tmp/kage-demo --id ws-main --backend fallback
```

Edit the fallback merged view:

```bash
echo changed > /tmp/kage-home/workspaces/ws-main/merged/README.md
echo new > /tmp/kage-home/workspaces/ws-main/merged/new.txt
```

Diff and commit back:

```bash
./target/debug/kage --home /tmp/kage-home workspace diff ws-main
./target/debug/kage --home /tmp/kage-home workspace commit ws-main -m "workspace changes"
```

Discard workspace state:

```bash
./target/debug/kage --home /tmp/kage-home workspace discard ws-main
```

## Workspace lifecycle

```bash
kage --home .kage workspace create --ref main --repo . --id ws-main --backend fallback
kage --home .kage workspace list
kage --home .kage workspace mount ws-main
kage --home .kage workspace diff ws-main
kage --home .kage workspace commit ws-main -m "message"
kage --home .kage workspace discard ws-main
kage --home .kage workspace gc
```

Workspace layout:

```text
.kage/workspaces/<id>/
  workspace.tsv   persisted metadata
  lower/          fallback read-only-ish exported tree; debug backend only
  upper/          changed files plus .kage/deleted metadata
  work/           backend work directory reserved per workspace
  merged/         fallback editable merged view
```

`upper/`, `work/`, and `merged/` are per-workspace and must not be shared.

## Diff and commit-back

`workspace diff` refreshes the upper layer from the fallback merged view, then asks Git to compare the parent commit to a synthetic tree made from:

```text
parent Git tree + upper files - .kage/deleted paths
```

`workspace commit`:

1. refreshes the upper layer from the fallback merged view;
2. verifies the target ref still points to the workspace parent commit;
3. builds a synthetic tree using a temporary Git index;
4. creates a commit with the workspace parent as parent;
5. updates the ref only after commit creation succeeds.

Supported by tests:

- added files;
- modified files;
- deleted files;
- rename represented as delete + add;
- executable bit preservation;
- symlink preservation;
- binary file preservation;
- stale ref rejection;
- detached/non-updatable ref rejection;
- empty diff rejection.

## Container and agent execution

Local command execution:

```bash
kage --home .kage exec ws-main -- cargo test
```

Container execution:

```bash
kage --home .kage run ws-main --image rust:latest -- cargo test
kage --home .kage run ws-main --podman --image rust:latest -- cargo test
kage --home .kage run ws-main --apple-container --image rust:latest -- cargo test
```

The current implementation constructs the container command and bind-mounts the prepared workspace at `/workspace`. It does not yet provision or verify the Apple managed Linux VM by itself. Manual Apple Container verification requires Apple Silicon macOS with the `container` CLI installed.

## Backends

`kage-overlay` exposes a `WorkspaceBackend` trait with these implemented choices:

- `fallback`: directory-merge fallback for unprivileged development and tests. It may materialize the selected tree into `lower/`, so it is not the production checkout-less backend. It stores deletions in `upper/.kage/deleted`.
- `overlayfs`: native Linux overlayfs backend. It validates distinct lower/upper/work/merged directories, requires an empty workdir, runs `mount -t overlay overlay -o lowerdir=<lower>,upperdir=<upper>,workdir=<work> <merged>`, and unmounts idempotently. It is opt-in with `--backend overlayfs` or `KAGE_BACKEND=overlayfs`.

Example:

```bash
kage --home .kage workspace create --ref main --repo . --id ws-main --backend overlayfs
# or
KAGE_BACKEND=overlayfs kage --home .kage workspace create --ref main --repo . --id ws-main
```

Overlayfs requires Linux, overlayfs support, and enough privilege/capability to mount overlay filesystems, typically root or `CAP_SYS_ADMIN`. In the current prototype, both fallback and overlayfs still use the exported `lower/` directory; the lazy Git read-only lower filesystem is future work.

Both backends:

- represent rename as delete + add;
- intentionally ignore empty directories because Git trees do not store them.

Not yet implemented:

- rootless fuse-overlayfs mount orchestration;
- daemonized mount supervision;
- Apple Container VM provisioning/health checks;
- concurrent registry locking beyond per-workspace directory isolation;
- rebase/merge/create-ref strategy for stale workspaces.

Environment-gated overlay tests:

```bash
KAGE_TEST_OVERLAY=1 cargo test -p kage-overlay overlayfs_detection_is_explicitly_environment_dependent -- --nocapture
KAGE_TEST_OVERLAY=1 cargo test -p kage-git overlayfs_backend_tree_matches_fallback_tree_when_enabled -- --nocapture
```

With `KAGE_TEST_OVERLAY=1`, the overlay integration test requires a real overlay mount and fails if the host lacks mount capability. For exploratory local runs where a permission-related skip is acceptable, set `KAGE_TEST_OVERLAY_ALLOW_SKIP=1`; the test prints an explicit warning before skipping the mount body. A real validation run should be performed on a Linux host or managed Linux VM with overlayfs mount capability.

A manual privileged verification helper is available:

```bash
scripts/verify-overlayfs.sh
```

It prints kernel, identity, overlay availability, tempdir filesystem details, a direct mount probe, and then runs `KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture`.

Xattr-based whiteout and opaque directory detection can be tested on filesystems that allow `trusted.overlay.*` xattrs and have `setfattr` installed:

```bash
KAGE_TEST_OVERLAY_XATTR=1 cargo test -p kage-git overlay_xattr_whiteout_and_opaque_directory_when_enabled -- --nocapture
```

## Safety model

- Ordinary workspace editing does not update Git refs.
- Commit-back rejects stale refs when the target ref advanced after workspace creation.
- Workspace IDs and workspace-relative paths are validated; absolute paths, `..`, and `.git` mutation paths are rejected.
- Command execution uses `std::process::Command` argument arrays rather than shell command strings where possible.
- Runtime metadata under `.kage` is excluded from synthetic Git trees.
- Caches and fallback directories are disposable; Git object store and upper layer metadata are the meaningful state.

## Tests

Default checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The default test suite uses temporary Git repositories and does not require network access. Overlayfs, container, and VM execution are environment-dependent. Overlayfs tests are gated with `KAGE_TEST_OVERLAY=1`; this mode is strict by default and fails if a real overlay mount cannot be performed. Real Docker/Podman/Apple Container execution should be manually verified on the target host.

## Development notes

- Do not use `git checkout` or `git worktree` as the primary workspace mechanism.
- Keep production commit-back based on parent Git tree + upper-layer mutations.
- Keep full directory materialization clearly labeled as fallback/debug/test infrastructure.
- Do not commit `.kage` runtime metadata.
- Preserve Git semantics for modes, symlinks, binary blobs, and ref update atomicity.
- Add tests that assert resulting Git object state, not only command success.
