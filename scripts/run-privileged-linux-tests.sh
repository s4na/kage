#!/usr/bin/env sh
set -eu

IMAGE="${KAGE_PRIVILEGED_IMAGE:-rust:1-bookworm}"
CMD='set -eux; rustup component add rustfmt clippy; cargo fmt --all -- --check; cargo clippy --workspace --all-targets --all-features -- -D warnings; cargo test --workspace --all-features; KAGE_TEST_ROFS=1 cargo test --workspace --all-features -- --nocapture; KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture; KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture'

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required for this privileged verification route" >&2
  echo "Install Docker or run the printed command on a Linux host with /dev/fuse and overlayfs mount capability." >&2
  exit 127
fi

echo "== privileged kage verification =="
echo "image: $IMAGE"
echo "requires: Docker, --privileged, /dev/fuse, overlayfs, and enough kernel capability for FUSE and overlay mounts"
echo "command: $CMD"

exec docker run --rm --privileged \
  --device /dev/fuse \
  -v "$PWD:/work" \
  -w /work \
  "$IMAGE" \
  bash -lc "$CMD"
