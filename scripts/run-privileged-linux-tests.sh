#!/usr/bin/env sh
set -eu

exec "$(dirname "$0")/run-docker-privileged-rofs-overlay-tests.sh" "$@"
