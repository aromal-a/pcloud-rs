#!/usr/bin/env bash
# Strict Linux kernel-mount and large-transfer release gate.

set -euo pipefail

command -v fusermount3 >/dev/null
command -v findmnt >/dev/null
test -c /dev/fuse
test -r /dev/fuse
test -w /dev/fuse

check_for_leaked_mounts() {
  local status=$?
  trap - EXIT
  if findmnt -rn -t fuse.pcloud,fuse.pcloud-rs | grep -q .; then
    echo "ERROR: pcloud-rs FUSE mount remains after the Linux release gate" >&2
    findmnt -rn -t fuse.pcloud,fuse.pcloud-rs >&2
    status=1
  fi
  exit "${status}"
}
trap check_for_leaked_mounts EXIT

# Run every practical ignored Linux FUSE mount/probe test serially. The 2 GiB
# case is intentionally separate: it exercises the chunked transfer pipeline,
# not a kernel mount, and has a distinct resource profile.
PCLOUD_FUSE_TEST=1 PCLOUD_STRICT_FUSE_TEST=1 \
  cargo test -p pcloud-fs --locked -- \
    --ignored \
    --skip chunked_flush_sustains_2gib_write_with_transient_retry \
    --nocapture \
    --test-threads=1

cargo test -p pcloud-fs \
  --test chunked_upload_write_multi_gib \
  --locked \
  chunked_flush_sustains_2gib_write_with_transient_retry \
  -- \
  --ignored \
  --exact \
  --nocapture

echo "Linux release mount gate: all practical kernel tests, 2 GiB stress, and cleanup passed"
