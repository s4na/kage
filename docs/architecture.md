# kage architecture

kage is a Git-native Copy-on-Write workspace runtime. The production architecture is parent Git tree + per-workspace upper layer + existing Linux overlay semantics. The current repository implements the Rust control plane and a non-privileged fallback backend that is suitable for tests and early review, but not the final production overlay backend.

## Components

- `kage-core`: metadata, registry paths, lower-kind metadata, path validation, and base-layer trait boundaries.
- `kage-rofs`: lazy read-only Git tree view/model using Git plumbing. It does not materialize the whole tree and does not yet provide the FUSE lowerdir mount.
- `kage-git`: direct Git tree/ref/commit orchestration through the Git CLI. It builds synthetic trees by applying upper-layer changes to the parent tree in a temporary index.
- `kage-overlay`: `WorkspaceBackend` boundary, directory-merge fallback, native Linux overlayfs backend, deletion metadata, layout validation, idempotent unmount, and gated overlayfs tests.
- `kage-container`: container runtime command construction for Docker, Podman, and Apple Container.
- `kage-cli`: user-facing orchestration.

## Production target

```text
Apple Silicon Mac host
  Rust control plane: kage-core + kage-git + kage-cli/kage-daemon future
  Apple Container / managed arm64 Linux VM
  Linux side: overlayfs/fuse-overlayfs workspace mount + container/agent execution
```

The host does not need to provide native macOS filesystem mounting for v1. macOS-native mount support is optional future work. Linux-side overlayfs/fuse-overlayfs is the intended production filesystem substrate.

## Current fallback

The directory-merge backend exports a selected Git tree into `lower/` and presents a mutable `merged/` directory. This is a fallback for unprivileged development and testing. `kage-rofs` now supplies a checkout-less Git tree model for lookup/read_dir/read_file/readlink, but the FUSE mount that would expose it as overlayfs lowerdir is future work. The Linux overlayfs backend validates lower/upper/work/merged directories and performs a native overlay mount when explicitly selected. Commit-back does not commit `merged/` directly; it uses `upper/` mutations and overlay whiteouts/deletion metadata to construct a synthetic Git tree from parent tree + upper mutations.

## Commit-back safety

Commit-back is intentionally explicit:

1. Resolve and persist parent commit at workspace creation.
2. Refresh upper-layer mutations from the active backend.
3. Reject commit if the target ref no longer points to the workspace parent.
4. Build a synthetic tree in a temporary Git index.
5. Create a commit object.
6. Update the ref with old-value protection.

Unsupported stale-workspace strategies such as rebase, merge, or create-new-ref are future work and should be explicit CLI options.

## Overlayfs test semantics

`KAGE_TEST_OVERLAY=1` is strict: a real overlay mount must succeed or the integration test fails. `KAGE_TEST_OVERLAY_ALLOW_SKIP=1` can be used for exploratory local runs to turn mount permission failures into explicit warnings. Xattr-specific whiteout and opaque directory coverage is gated behind `KAGE_TEST_OVERLAY_XATTR=1`.

## Rofs test semantics

`KAGE_TEST_ROFS=1` is strict: a real read-only rofs mount must exist or the mount test fails. `KAGE_TEST_ROFS_ALLOW_SKIP=1` can be used for exploratory local runs to turn the current unimplemented FUSE mount into an explicit warning. Pure GitTreeView tests run by default and do not require FUSE.
