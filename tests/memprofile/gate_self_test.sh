#!/usr/bin/env bash
# tests/memprofile/gate_self_test.sh
#
# Bash self-test for the `compare_against_baseline` gate function in
# `tools/memprofile/run.sh`. We do NOT install or invoke heaptrack;
# instead we exercise the pure gate logic directly with synthetic
# `peak_rss_bytes` values against synthetic baseline JSON files.
#
# The five cases below pin:
#   1. Boundary at exactly +10% (110_000_000 vs baseline 100_000_000) PASSES.
#      The implementation uses `peak > threshold` (strict), so a peak
#      sitting exactly at +10% is NOT a regression. The plan-spec phrase
#      "≥10% trigger" is implemented as "> +10%". This boundary is the
#      most important property to pin so it cannot drift.
#   2. One byte over the +10% boundary (110_000_001) FAILS with exit 1.
#   3. An improvement (peak below baseline) PASSES with exit 0.
#   4. Cold-start (no baseline file) initialises the baseline and PASSES
#      with exit 0.
#   5. Update-baseline / compare with malformed baseline returns 3.
#
# Dependencies: bash, jq. No heaptrack, no cargo, no daemon.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUN_SH="${REPO_ROOT}/tools/memprofile/run.sh"

if [[ ! -f "${RUN_SH}" ]]; then
  echo "[self-test] cannot find ${RUN_SH}" >&2
  exit 2
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "[self-test] jq is required" >&2
  exit 2
fi

# Source the gate function without triggering the live profiling driver.
export PCLOUD_MEMPROFILE_SOURCE_ONLY=1
# The driver checks Linux-only; sourcing must NOT trip that check, since
# the sentinel returns before the platform gate. Confirm that with set +e.
set +e
# shellcheck disable=SC1090
source "${RUN_SH}"
src_rc=$?
set -e
if (( src_rc != 0 )); then
  echo "[self-test] failed to source ${RUN_SH} (rc=${src_rc})" >&2
  exit 2
fi
unset PCLOUD_MEMPROFILE_SOURCE_ONLY

if ! declare -F compare_against_baseline >/dev/null; then
  echo "[self-test] compare_against_baseline not exported by run.sh" >&2
  exit 2
fi

WORK_DIR="$(mktemp -d -t memprofile-self-test-XXXXXX)"
trap 'rm -rf "${WORK_DIR}"' EXIT

PASS=0
FAIL=0

# Helper: write a fresh baseline.json with a given peak_rss_bytes.
write_baseline() {
  local path="$1"
  local peak="$2"
  jq -n \
    --argjson peak "${peak}" \
    '{
       schema: "pcloud-rs/memprofile/baseline-v1",
       peak_rss_bytes: $peak,
       total_allocations: 1,
       run_duration_secs: 60,
       recorded_at: "1970-01-01T00:00:00Z"
     }' >"${path}"
}

# Helper: run compare_against_baseline, capture exit code, suppress stdout.
run_gate() {
  local peak="$1"
  local allocs="$2"
  local baseline_path="$3"
  set +e
  compare_against_baseline "${peak}" "${allocs}" "${baseline_path}" \
    >/dev/null 2>&1
  local rc=$?
  set -e
  printf '%d' "${rc}"
}

# Helper: assert a case.
assert_eq() {
  local label="$1"
  local got="$2"
  local want="$3"
  if [[ "${got}" == "${want}" ]]; then
    echo "[PASS] ${label} (rc=${got})"
    PASS=$(( PASS + 1 ))
  else
    echo "[FAIL] ${label} (got rc=${got}, want rc=${want})"
    FAIL=$(( FAIL + 1 ))
  fi
}

# --- Case 1: boundary at exactly +10% --------------------------------------
case1_baseline="${WORK_DIR}/case1_baseline.json"
write_baseline "${case1_baseline}" 100000000
rc1="$(run_gate 110000000 12345 "${case1_baseline}")"
assert_eq "case 1: peak=110_000_000 (exactly +10%) → PASS" "${rc1}" "0"

# --- Case 2: one byte over the +10% boundary -------------------------------
case2_baseline="${WORK_DIR}/case2_baseline.json"
write_baseline "${case2_baseline}" 100000000
rc2="$(run_gate 110000001 12345 "${case2_baseline}")"
assert_eq "case 2: peak=110_000_001 (>+10%) → FAIL (regression)" "${rc2}" "1"

# --- Case 3: improvement (peak below baseline) -----------------------------
case3_baseline="${WORK_DIR}/case3_baseline.json"
write_baseline "${case3_baseline}" 100000000
rc3="$(run_gate 99000000 12345 "${case3_baseline}")"
assert_eq "case 3: peak=99_000_000 (improvement) → PASS" "${rc3}" "0"

# --- Case 4: cold-start (baseline file does not exist yet) -----------------
case4_baseline="${WORK_DIR}/case4_baseline.json"
[[ ! -e "${case4_baseline}" ]] || rm -f "${case4_baseline}"
rc4="$(run_gate 100000000 9999 "${case4_baseline}")"
assert_eq "case 4: no baseline → PASS (cold-start initialises)" "${rc4}" "0"
if [[ -f "${case4_baseline}" ]]; then
  written_peak="$(jq -r '.peak_rss_bytes' "${case4_baseline}")"
  if [[ "${written_peak}" == "100000000" ]]; then
    echo "[PASS] case 4: cold-start wrote peak_rss_bytes=100000000"
    PASS=$(( PASS + 1 ))
  else
    echo "[FAIL] case 4: cold-start wrote peak_rss_bytes=${written_peak} (want 100000000)"
    FAIL=$(( FAIL + 1 ))
  fi
else
  echo "[FAIL] case 4: cold-start did not create ${case4_baseline}"
  FAIL=$(( FAIL + 1 ))
fi

# --- Case 5: malformed baseline (peak_rss_bytes missing) → exit 3 ---------
case5_baseline="${WORK_DIR}/case5_baseline.json"
echo '{"schema":"pcloud-rs/memprofile/baseline-v1"}' >"${case5_baseline}"
rc5="$(run_gate 100000000 1 "${case5_baseline}")"
assert_eq "case 5: malformed baseline (no peak_rss_bytes) → RUNTIME (rc=3)" "${rc5}" "3"

# --- Summary ---------------------------------------------------------------
TOTAL=$(( PASS + FAIL ))
echo
echo "[summary] ${PASS}/${TOTAL} passed, ${FAIL} failed"
if (( FAIL == 0 )); then
  echo "[summary] gate self-test: OK"
  exit 0
else
  echo "[summary] gate self-test: FAILURES"
  exit 1
fi
