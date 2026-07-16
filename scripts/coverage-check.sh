#!/usr/bin/env bash
# coverage-check.sh — gate a build on a line-coverage floor.
#
# Usage:
#   scripts/coverage-check.sh <lcov-path> <floor-percent>
#
# Reads an lcov.info file, sums the LF (lines found) and LH (lines hit)
# records across all source files, computes percent = LH / LF * 100, and
# exits non-zero unless the result is strictly greater than
# <floor-percent>. It also enforces the security-critical per-crate floors documented in
# docs/book/src/development/testing.md.
#
# Exit codes:
#   0 — coverage strictly above the requested percentage
#   1 — coverage at or below the requested percentage (gate fails)
#   2 — bad arguments / file missing / no LF records (parse failure)
#
# Invoked by `cargo xtask coverage`; contributors and release operators use
# the same parser and cannot drift from the local CI decision.

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <lcov-path> <floor-percent>" >&2
  exit 2
fi

LCOV="$1"
FLOOR="$2"

if [[ ! -f "$LCOV" ]]; then
  echo "error: lcov file not found: $LCOV" >&2
  exit 2
fi

# Sum LF (lines found) and LH (lines hit) across the whole report. Keep the
# raw counts for an exact comparison instead of rounding to an integer.
read -r LH LF PCT <<EOF
$(awk -F: '
  /^LF:/ { lf += $2 }
  /^LH:/ { lh += $2 }
  END {
    if (lf == 0) { print "NA NA NA"; exit }
    printf "%d %d %.2f", lh, lf, (lh * 100.0) / lf
  }
' "$LCOV")
EOF

if [[ "$LF" == "NA" ]]; then
  echo "error: no LF records in $LCOV (no instrumented lines)" >&2
  exit 2
fi

echo "coverage: ${PCT}% (${LH}/${LF}; required: > ${FLOOR}%)"

if ! awk -v lh="$LH" -v lf="$LF" -v floor="$FLOOR" \
  'BEGIN { exit !((lh * 100.0) > (floor * lf)) }'; then
  echo "FAIL: line coverage ${PCT}% is not strictly above ${FLOOR}%" >&2
  exit 1
fi

echo "OK: line coverage ${PCT}% is strictly above ${FLOOR}%"

# Keep the security-critical floors in this one shared parser so local and CI
# decisions cannot drift. LCOV records are grouped by SF:/LF:/LH: entries.
if ! awk '
  BEGIN {
    floor["pcloud-secret"] = 90
    floor["pcloud-crypto"] = 85
    floor["pcloud-auth"] = 85
    floor["pcloud-resilience"] = 80
    floor["pcloud-ipc"] = 80
  }
  /^SF:/ {
    source = substr($0, 4)
    current = ""
    for (name in floor) {
      marker = "/crates/" name "/"
      if (index(source, marker) || index(source, "crates/" name "/") == 1) {
        current = name
        seen[name] = 1
        break
      }
    }
    next
  }
  /^LF:/ && current != "" { found[current] += substr($0, 4) + 0; next }
  /^LH:/ && current != "" { hit[current] += substr($0, 4) + 0; next }
  END {
    failed = 0
    for (name in floor) {
      if (!seen[name] || found[name] == 0) {
        printf "FAIL: no coverage records found for security-critical crate %s\n", name > "/dev/stderr"
        failed = 1
        continue
      }
      pct = (hit[name] * 100.0) / found[name]
      printf "critical coverage: %s %.2f%% (%d/%d; floor %d%%)\n", \
        name, pct, hit[name], found[name], floor[name]
      if (pct + 0.000001 < floor[name]) {
        printf "FAIL: %s line coverage %.2f%% is below floor %d%%\n", \
          name, pct, floor[name] > "/dev/stderr"
        failed = 1
      }
    }
    exit failed
  }
' "$LCOV"; then
  exit 1
fi

echo "OK: all security-critical crate coverage floors are met"
exit 0
