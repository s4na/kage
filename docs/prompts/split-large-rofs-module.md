# Prompt: split the large kage-rofs module safely

Refactor `crates/kage-rofs/src/lib.rs`, which currently exceeds 1,000 lines, into cohesive Rust modules without changing public behavior.

## Constraints

- Treat behavior preservation as the top priority: run the existing `kage-rofs` unit tests before and after the split, and keep mount-free protocol tests passing.
- Keep every touched source file under 1,000 lines so future LLM-assisted edits stay reliable.
- Preserve the public crate API: `GitEntryKind`, `GitMetadata`, `GitTreeView`, `Result`, `mount_rofs_strict`, `rofs_mount_available`, `RofsBackend`, and `RofsMount` must remain exported from the crate root.
- Prefer mechanical moves over rewrites. Only widen visibility to `pub(crate)` where cross-module collaboration requires it.
- Split by responsibility:
  - `git_tree.rs`: Git tree lookup, metadata, blob/symlink reads, and Git command helpers.
  - `fuse.rs`: handwritten FUSE request handling, inode table, reply packing, and mount-free protocol tests.
  - `mount.rs`: backend selection, mount handle lifecycle, and backend dispatch.
  - `sys.rs`: direct FUSE/fusermount3 syscalls, constants, FFI bindings, and low-level helpers.
  - `lib.rs`: module declarations and public re-exports only.

## Sub-agent plan

1. Ask an explorer agent to map the original giant file's responsibilities, public API, and safe module boundaries.
2. Ask a second explorer agent to inspect the relevant test commands and behavior-focused coverage.
3. Implement the split locally, using the explorers as context rather than letting one agent perform a broad rewrite.
4. Run `cargo test -p kage-rofs`, address compile/test regressions, then run formatting and workspace checks.
