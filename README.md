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

- `kage-core`: workspace metadata, runtime paths, path validation, lower-kind metadata, and the read-only `BaseLayer` trait boundary.
- `kage-rofs`: lazy read-only Git tree model. It resolves refs/commits and reads tree entries/blobs/symlinks directly through Git plumbing without checkout/archive/export. It does not yet provide a FUSE mount.
- `kage-git`: Git CLI backed object/ref adapter. Commit-back applies the workspace upper layer to the parent tree through a temporary Git index instead of scanning a normal Git worktree.
- `kage-overlay`: backend trait plus two implemented backends: non-privileged `fallback` directory-merge for debug/tests, and opt-in `overlayfs` for native Linux overlay mounts.
- `kage-container`: container argv construction for Docker, Podman, and Apple Container.
- `kage-cli`: CLI for repository/workspace lifecycle, diff, commit, local exec, and container exec.

The fallback backend still exports a Git tree into an internal lower directory to provide a plain editable `merged` directory in unprivileged environments. That fallback is development/debug infrastructure. The first production-like backend boundary is `overlayfs`, which runs a native Linux overlay mount over lower/upper/work/merged directories when explicitly selected. `kage-rofs` now provides the lazy Git tree view/model, but the actual read-only FUSE mount that would let overlayfs use it as `lowerdir` is not implemented yet.

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
./target/debug/kage --home /tmp/kage-home workspace create --ref main --repo /tmp/kage-demo --id ws-main --backend fallback --lower exported
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
kage --home .kage workspace create --ref main --repo . --id ws-main --backend fallback --lower exported
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

`kage-overlay` exposes a `WorkspaceBackend` trait with these implemented choices. Workspace creation also accepts a lower source via `--lower` or `KAGE_LOWER`:

- `fallback`: directory-merge fallback for unprivileged development and tests. It may materialize the selected tree into `lower/`, so it is not the production checkout-less backend. It stores deletions in `upper/.kage/deleted`.
- `overlayfs`: native Linux overlayfs backend. It validates distinct lower/upper/work/merged directories, requires an empty workdir, runs `mount -t overlay overlay -o lowerdir=<lower>,upperdir=<upper>,workdir=<work> <merged>`, and unmounts idempotently. It is opt-in with `--backend overlayfs` or `KAGE_BACKEND=overlayfs`.
- `--lower exported`: default lower source. It uses the exported lower fallback and is the only lower that can currently be mounted.
- `--lower git-rofs`: lazy Git tree model. It can read Git tree contents without checkout/export, but workspace mounting with it is rejected until the read-only FUSE mount is implemented.

Example:

```bash
kage --home .kage workspace create --ref main --repo . --id ws-main --backend overlayfs --lower exported
# or
KAGE_BACKEND=overlayfs KAGE_LOWER=exported kage --home .kage workspace create --ref main --repo . --id ws-main

# GitTreeView-only lower model; currently fails for workspace mount until rofs FUSE exists:
kage --home .kage workspace create --ref main --repo . --id ws-rofs --backend overlayfs --lower git-rofs
```

Overlayfs requires Linux, overlayfs support, and enough privilege/capability to mount overlay filesystems, typically root or `CAP_SYS_ADMIN`. In the current prototype, mounted workspaces still require `--lower exported`; `--lower git-rofs` is available as a lazy read-only Git tree model but not yet as a filesystem `lowerdir`.

Both backends:

- represent rename as delete + add;
- intentionally ignore empty directories because Git trees do not store them.

Not yet implemented:

- read-only rofs FUSE mount;
- rofs + overlayfs strict integration;
- rootless fuse-overlayfs mount orchestration;
- daemonized mount supervision;
- Apple Container VM provisioning/health checks;
- concurrent registry locking beyond per-workspace directory isolation;
- rebase/merge/create-ref strategy for stale workspaces.

Environment-gated rofs and overlay tests:

```bash
KAGE_TEST_ROFS=1 cargo test -p kage-rofs rofs_mount_strict_fails_until_fuse_mount_is_implemented -- --nocapture
KAGE_TEST_OVERLAY=1 cargo test -p kage-overlay overlayfs_detection_is_explicitly_environment_dependent -- --nocapture
KAGE_TEST_OVERLAY=1 cargo test -p kage-git overlayfs_backend_tree_matches_fallback_tree_when_enabled -- --nocapture
KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture
```

With `KAGE_TEST_OVERLAY=1`, the overlay integration test requires a real overlay mount and fails if the host lacks mount capability. For exploratory local runs where a permission-related skip is acceptable, set `KAGE_TEST_OVERLAY_ALLOW_SKIP=1`; the test prints an explicit warning before skipping the mount body. A real validation run should be performed on a Linux host or managed Linux VM with overlayfs mount capability.

Manual verification helpers are available:

```bash
scripts/verify-rofs.sh
scripts/verify-overlayfs.sh
scripts/verify-rofs-overlay.sh
```

They print kernel/OS/architecture, identity, `/dev/fuse`, overlay availability, tempdir filesystem details, direct mount probes where applicable, and strict gated cargo test commands.

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

The default test suite uses temporary Git repositories and does not require network access. Pure `GitTreeView` tests run by default. Rofs mount, overlayfs, container, and VM execution are environment-dependent. `KAGE_TEST_ROFS=1` and `KAGE_TEST_OVERLAY=1` are strict by default and fail if real mounts cannot be performed. Real Docker/Podman/Apple Container execution should be manually verified on the target host.

## Development notes

- Do not use `git checkout` or `git worktree` as the primary workspace mechanism.
- Keep production commit-back based on parent Git tree + upper-layer mutations.
- Keep full directory materialization clearly labeled as fallback/debug/test infrastructure.
- Do not commit `.kage` runtime metadata.
- Preserve Git semantics for modes, symlinks, binary blobs, and ref update atomicity.
- Add tests that assert resulting Git object state, not only command success.
