# FUSE backend evaluation for kage-rofs

Decision: **USE_FUSER_FOR_REAL_MOUNT_BACKEND**

The current failure is in the FUSE mount/session/request serving lifecycle. The
GitTreeView model and mount-free protocol tests already cover read-only tree
semantics. The next production candidate should therefore minimize custom FUSE
session code and keep the GitTreeView surface narrow.

## fuser

- Mount/session lifecycle: provides a Rust `Filesystem` trait and mount helpers,
  avoiding handwritten `_FUSE_COMMFD`, socketpair, `recvmsg`, and raw request loop
  code in kage.
- fusermount3/rootless support: intended to work through the normal Linux FUSE
  helper path where available, matching the route GitHub Actions now exercises.
- Background serving API: supports session/mount lifecycle APIs suitable for a
  mount handle owned by `RofsMount`.
- Operation coverage: directly maps to the needed read-only operations:
  `lookup`, `getattr`, `open`, `read`, `opendir`, `readdir`, `readlink`,
  `access`, and `statfs`.
- Error handling: replies use normal errno values, which should make read-only
  mutation failures (`EROFS`) and missing paths (`ENOENT`) clearer than raw reply
  packing.
- Unmount/drop behavior: library-owned session objects should make teardown less
  error-prone than closing a raw fd plus joining a handwritten worker.
- Dependency impact: adds one maintained Rust FUSE dependency and its transitive
  crates.
- Async runtime requirement: none expected for the narrow synchronous read-only
  backend.
- Suitability for GitTreeView: high. `GitTreeView` operations are synchronous
  and map cleanly to fuser callbacks.

## fuse3

- Mount/session lifecycle: also library-backed, but its API is more oriented
  toward async usage.
- fusermount3/rootless support: suitable in principle, but would require more
  integration work to fit the current synchronous `GitTreeView` API.
- Background serving API: likely needs runtime/task coordination.
- Operation coverage: supports the required operations.
- Error handling: structured errno replies are available.
- Unmount/drop behavior: library-managed, but async lifecycle integration would
  add more moving pieces.
- Dependency impact: potentially larger due to async ecosystem requirements.
- Async runtime requirement: likely, which is unnecessary for kage-rofs' current
  read-only synchronous Git tree model.
- Suitability for GitTreeView: good but less direct than fuser.

## Current handwritten backend

- Mount/session lifecycle: currently fragile. Direct mount returns `EINVAL`; the
  helper path can return an fd and produce a visible mount, but reads hang.
- fusermount3/rootless support: partially implemented but not sufficient for
  usable mounted I/O.
- Background serving API: custom worker thread around a raw fd.
- Operation coverage: mount-free tests cover lookup/getattr/read/readlink/readdir
  and read-only mutation errors, but strict kernel I/O hangs.
- Error handling: manual binary reply construction makes protocol mistakes hard
  to diagnose.
- Unmount/drop behavior: custom fd close/unmount/thread join path.
- Dependency impact: no external dependency, but high maintenance and correctness
  burden.
- Async runtime requirement: none.
- Suitability for GitTreeView: acceptable for model tests, not acceptable as the
  default real-mount backend after the current hang evidence.

## Required backend direction

Use fuser for the real-mount backend. Keep the handwritten backend isolated for
mount-free protocol/model tests and explicit legacy diagnostics only. Do not
silently fall back from fuser to handwritten when `KAGE_ROFS_BACKEND=fuser` is
selected.
