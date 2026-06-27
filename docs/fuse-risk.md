# kage-rofs FUSE risk note

`kage-rofs` currently uses a narrow hand-written FUSE protocol implementation rather than `fuser`.

## Why `fuser` is not used yet

The preferred direction is to replace the manual FUSE protocol layer with a maintained Rust FUSE crate such as `fuser`. During convergence in the current environment, dependency fetching from crates.io failed with proxy HTTP 403 errors, and package installation through apt also failed with HTTP 403 errors. Because the repository still needed mount-free verification below the kernel boundary, the hand-written implementation remains for now and is covered by protocol-level tests.

This is a temporary reviewable boundary, not a production-readiness claim.

## Supported opcodes

The read-only server intentionally implements only the operations required for a Git tree lower layer:

- `FUSE_INIT`
- `FUSE_LOOKUP`
- `FUSE_GETATTR`
- `FUSE_OPEN`
- `FUSE_OPENDIR`
- `FUSE_READ`
- `FUSE_READDIR`
- `FUSE_READLINK`
- `FUSE_STATFS`
- `FUSE_RELEASE`
- `FUSE_RELEASEDIR`
- `FUSE_FLUSH`
- `FUSE_FORGET` (no reply)
- `FUSE_DESTROY` (no reply)

## Intentionally unsupported or read-only operations

Mutating operations return `EROFS`:

- `FUSE_SETATTR`
- `FUSE_MKDIR`
- `FUSE_UNLINK`
- `FUSE_RMDIR`
- `FUSE_RENAME`
- `FUSE_WRITE`
- `FUSE_CREATE`
- `FUSE_SYMLINK`
- `FUSE_LINK`

Other unrecognized opcodes return `ENOSYS`.

## Remaining ABI and unsafe risk

The risky areas are:

- manual FUSE struct layout and offsets;
- native-endian request/response parsing;
- `unsafe extern "C"` bindings for `open`, `mount`, `umount2`, `read`, and `write`;
- mount option compatibility across kernels;
- lifecycle behavior under real kernel request ordering and interruption.

## Mount-free test coverage

Default tests cover the protocol methods directly without `/dev/fuse`:

- init reply shape;
- stable inode lookup;
- getattr modes and sizes;
- full, offset, short, EOF, and binary reads;
- exact symlink readlink payload;
- root and nested readdir, including offset continuation;
- Unicode and space-containing names;
- `FUSE_FORGET` no-reply behavior;
- read-only `EROFS` mutation errors;
- large directory and large file reads;
- no Git object mutation during reads.

## Strict kernel proof still required

Production readiness requires strict tests on a capable Linux host/VM:

```bash
KAGE_TEST_ROFS=1 cargo test --workspace --all-features -- --nocapture
KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture
KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1 cargo test --workspace --all-features -- --nocapture
```

Allow-skip variants are useful for local diagnostics but are not proof.

## Current mount mechanism and helper gap

`kage-rofs` first opens `/dev/fuse` directly and calls the kernel `mount(2)` path with filesystem type `fuse` and `fd=...` mount options. That direct mount path generally requires root or `CAP_SYS_ADMIN`. If direct mounting fails and `fusermount3` is available, `kage-rofs` now falls back to the libfuse-style helper route that receives the `/dev/fuse` file descriptor over `_FUSE_COMMFD`. On GitHub-hosted runners this distinction matters: `/dev/fuse` can exist and be writable while the non-root direct `mount(2)` call still fails with `Operation not permitted`.

The CI probe therefore treats non-sudo direct mount failure as insufficient evidence for a true environment impossibility. It separately probes `fusermount3`, `fuse-overlayfs`, passwordless `sudo`, sudo capabilities, non-sudo overlay, sudo overlay, rootless `fuse-overlayfs`, and sudo `fuse-overlayfs`. The helper fallback still needs Level 2/4 GitHub-hosted proof before it can be considered runtime-ready.
