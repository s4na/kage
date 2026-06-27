# kage-rofs FUSE backend status

Current strict host and privileged-container artifacts show that the blocker is
not broad filesystem setup. Both routes have usable `/dev/fuse` plus overlay
substrate probes, and overlay strict tests pass independently. Level 2 is still
unproven because the mounted read-only filesystem is not usable for kernel I/O.

Observed rofs failure shape:

- Direct FUSE mount returns `EINVAL` on host and container routes.
- The `fusermount3` helper fallback is attempted.
- `_FUSE_COMMFD` is set and the helper process is spawned.
- Receiving the FUSE file descriptor from the helper succeeds.
- `findmnt` sees the mountpoint as `fuse.kage-rofs`.
- Reading `README.md` through the mounted filesystem hangs.
- The strict rofs test's internal body timeout fires after 20 seconds.

This means mount setup is partially working: the helper can create a visible
FUSE mount and hand kage-rofs a session file descriptor. The failing boundary is
the serving/session/request lifecycle after mount, not `GitTreeView` semantics
alone and not just access to `/dev/fuse`.

The current handwritten backend remains valuable for mount-free protocol/model
tests, but it should be treated as a legacy strict-mount implementation until it
can answer kernel requests without hanging. A production candidate real-mount
backend should use a maintained FUSE library so mount helper behavior, FUSE_INIT,
request dispatch, background serving, and unmount/drop behavior are handled by
well-tested code.

Proof status:

- Host overlay substrate: proven independently in current artifacts.
- Containerized overlay substrate: proven independently in current artifacts.
- Level 2 rofs: unproven, because mounted filesystem reads do not complete.
- Level 3 combined rofs+overlay: unproven, because rofs never becomes a usable
  lower filesystem.
- Level 4 runtime: unproven for the same rofs mounted-I/O reason.
