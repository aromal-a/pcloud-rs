#!/usr/bin/env bash
# tools/memprofile/run.sh
#
# T3.6 — Memory profiling driver.
#
# Builds pcloudd in release mode, spins up a hermetic dev-mode profile,
# runs `heaptrack pcloudd` for `RUN_DURATION_SECS` seconds while
# synthesising sync activity, and then compares peak RSS / total
# allocations against the recorded baseline at
# `tools/memprofile/baseline.json`.
#
# Modes:
#   - default (CI):   compare against baseline, fail on >=10% RSS regression.
#   - --update-baseline (operator-only): write a new baseline JSON.
#
# Environment:
#   RUN_DURATION_SECS  Seconds to keep heaptrack alive. Default: 60.
#                      CI workflow sets 900 (15 min). Production 24h soak
#                      is operator-driven via `workflow_dispatch` with
#                      RUN_DURATION_SECS=86400.
#   PCLOUD_BIN_DIR     Directory containing the freshly-built `pcloudd`.
#                      Default: ./target/release.
#   MEMPROFILE_OUT_DIR Where to drop heaptrack.json + the .heaptrack raw
#                      capture. Default: ./memprofile-out.
#
# Exit codes:
#   0   PASS — baseline exists and current peak RSS within +10% of baseline,
#        OR baseline was just initialised (cold start), OR --update-baseline.
#   1   FAIL — peak RSS regressed >=10% versus baseline.
#   2   USAGE — bad arguments, missing tools, or hermetic profile setup
#        failed before heaptrack started.
#   3   RUNTIME — heaptrack itself failed, or post-processing (jq /
#        heaptrack_print) could not extract the metrics.
#
# This script is **Linux-only**: heaptrack is a Linux-only tool. The CI
# job runs on `ubuntu-latest`. On non-Linux hosts the script exits 2
# with a documented "platform not supported" message.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
BASELINE_FILE="${SCRIPT_DIR}/baseline.json"
REGRESSION_THRESHOLD_PCT="${REGRESSION_THRESHOLD_PCT:-10}"

# --- Gate function (shared with tests/memprofile/gate_self_test.sh) ------
#
# compare_against_baseline <peak_rss_bytes> <total_allocations> <baseline_path>
#
# Pure gate logic factored out so the Bash self-test can exercise it
# without invoking heaptrack. Behaviour matches the inline branch below:
#
#   - If <baseline_path> does not exist:
#         write a fresh baseline JSON containing peak_rss_bytes /
#         total_allocations / run_duration_secs / recorded_at, print a
#         cold-start message, and return 0.
#   - If the baseline exists and its `peak_rss_bytes` is missing/null,
#     return 3 (RUNTIME).
#   - Otherwise compute THRESHOLD = baseline * (100 + REGRESSION_THRESHOLD_PCT)
#     / 100 with integer math. If peak > THRESHOLD return 1 (FAIL),
#     else return 0 (PASS).
#
# `RUN_DURATION_SECS` is read from the environment for cold-start writes;
# it defaults to 0 when unset (the self-test exercises this).
compare_against_baseline() {
  local peak_rss_bytes="$1"
  local total_allocations="$2"
  local baseline_path="$3"
  local run_secs="${RUN_DURATION_SECS:-0}"

  local current_json
  current_json="$(jq -n \
    --argjson peak_rss "${peak_rss_bytes}" \
    --argjson total_allocs "${total_allocations}" \
    --arg run_secs "${run_secs}" \
    --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{
       schema: "pcloud-rs/memprofile/baseline-v1",
       peak_rss_bytes: $peak_rss,
       total_allocations: $total_allocs,
       run_duration_secs: ($run_secs | tonumber),
       recorded_at: $recorded_at
     }')"

  if [[ ! -f "${baseline_path}" ]]; then
    echo "${current_json}" | jq '.' >"${baseline_path}"
    echo "[memprofile] no prior baseline — initialised ${baseline_path}"
    echo "[memprofile] re-run on a future build to gate against this baseline."
    return 0
  fi

  local baseline_peak_rss
  baseline_peak_rss="$(jq -r '.peak_rss_bytes' "${baseline_path}")"
  if [[ -z "${baseline_peak_rss}" || "${baseline_peak_rss}" == "null" ]]; then
    echo "[memprofile] baseline ${baseline_path} is missing peak_rss_bytes" >&2
    return 3
  fi

  local threshold_rss
  threshold_rss=$(( baseline_peak_rss * (100 + REGRESSION_THRESHOLD_PCT) / 100 ))

  echo "[memprofile] baseline peak RSS:  ${baseline_peak_rss} bytes"
  echo "[memprofile] threshold (+${REGRESSION_THRESHOLD_PCT}%):     ${threshold_rss} bytes"
  echo "[memprofile] current peak RSS:   ${peak_rss_bytes} bytes"

  if (( peak_rss_bytes > threshold_rss )); then
    echo "[memprofile] FAIL: peak RSS regressed >=${REGRESSION_THRESHOLD_PCT}% vs baseline." >&2
    echo "[memprofile] If this regression is intended, re-run with --update-baseline." >&2
    return 1
  fi

  echo "[memprofile] PASS: peak RSS within +${REGRESSION_THRESHOLD_PCT}% of baseline."
  return 0
}

# Sourced-only mode: when this script is sourced (not executed directly)
# with PCLOUD_MEMPROFILE_SOURCE_ONLY=1, expose `compare_against_baseline`
# and skip the live profiling driver below. This lets the self-test
# import the function without triggering the heaptrack-dependent path.
if [[ "${PCLOUD_MEMPROFILE_SOURCE_ONLY:-0}" == "1" ]]; then
  return 0 2>/dev/null || exit 0
fi

UPDATE_BASELINE=0
for arg in "$@"; do
  case "${arg}" in
    --update-baseline)
      UPDATE_BASELINE=1
      ;;
    -h|--help)
      sed -n '1,40p' "$0"
      exit 0
      ;;
    *)
      echo "[memprofile] unknown argument: ${arg}" >&2
      exit 2
      ;;
  esac
done

# --- Platform + dependency gate -------------------------------------------
case "$(uname -s)" in
  Linux) ;;
  *)
    echo "[memprofile] heaptrack is Linux-only; this host is $(uname -s)." >&2
    echo "[memprofile] See docs/book/src/operations/memory-profiling.md." >&2
    exit 2
    ;;
esac

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "[memprofile] required tool not found: $1" >&2
    echo "[memprofile] install it (e.g. 'apt-get install $2') and retry." >&2
    exit 2
  fi
}
require_tool heaptrack heaptrack
require_tool heaptrack_print heaptrack
require_tool jq jq
require_tool cargo "rust toolchain"

RUN_DURATION_SECS="${RUN_DURATION_SECS:-60}"
PCLOUD_BIN_DIR="${PCLOUD_BIN_DIR:-${REPO_ROOT}/target/release}"
OUT_DIR="${MEMPROFILE_OUT_DIR:-${REPO_ROOT}/memprofile-out}"

mkdir -p "${OUT_DIR}"

# --- Build pcloudd in release mode ---------------------------------------
echo "[memprofile] building pcloudd (release)…"
( cd "${REPO_ROOT}" && cargo build --release -p pcloud-daemon )

PCLOUDD_BIN="${PCLOUD_BIN_DIR}/pcloudd"
if [[ ! -x "${PCLOUDD_BIN}" ]]; then
  echo "[memprofile] pcloudd not found at ${PCLOUDD_BIN}" >&2
  exit 2
fi

# --- Hermetic dev-mode profile ------------------------------------------
HERMETIC_DIR="$(mktemp -d -t memprofile-hermetic-XXXXXX)"
SYNC_ROOT="${HERMETIC_DIR}/sync-root"
STATE_DIR="${HERMETIC_DIR}/state"
RUNTIME_DIR="${HERMETIC_DIR}/runtime"
CONFIG_FILE="${HERMETIC_DIR}/pcloudd.toml"
mkdir -p "${SYNC_ROOT}" "${STATE_DIR}" "${RUNTIME_DIR}"
chmod 700 "${HERMETIC_DIR}" "${STATE_DIR}" "${RUNTIME_DIR}"

cat >"${CONFIG_FILE}" <<EOF
# Hermetic memory-profiling profile. NOT a production config.
#
# Mirrors the shape of ConfigProfile::secure_defaults but redirects all
# state/runtime/config dirs to a tempdir so the run is fully isolated
# and leaves no host residue.

[profile]
mode = "dev"

[paths]
state_dir   = "${STATE_DIR}"
runtime_dir = "${RUNTIME_DIR}"
config_dir  = "${HERMETIC_DIR}"

[transport]
# Dev mode: no network. Heaptrack measures the daemon's resident behaviour,
# not pCloud-server interactions.
mode = "offline"

[ipc]
# Owner-only socket inside the hermetic runtime dir.
socket = "${RUNTIME_DIR}/pcloudd.sock"
EOF

cleanup() {
  if [[ -n "${HEAPTRACK_PID:-}" ]] && kill -0 "${HEAPTRACK_PID}" 2>/dev/null; then
    kill -TERM "${HEAPTRACK_PID}" 2>/dev/null || true
    wait "${HEAPTRACK_PID}" 2>/dev/null || true
  fi
  rm -rf "${HERMETIC_DIR}"
}
trap cleanup EXIT INT TERM

# --- Run heaptrack -------------------------------------------------------
RAW_CAPTURE="${OUT_DIR}/pcloudd.heaptrack"
JSON_REPORT="${OUT_DIR}/heaptrack.json"
rm -f "${RAW_CAPTURE}" "${JSON_REPORT}"

echo "[memprofile] running heaptrack for ${RUN_DURATION_SECS}s…"
heaptrack -o "${RAW_CAPTURE}" "${PCLOUDD_BIN}" --config "${CONFIG_FILE}" \
  >"${OUT_DIR}/pcloudd.stdout" 2>"${OUT_DIR}/pcloudd.stderr" &
HEAPTRACK_PID=$!

# --- Synthesise sync activity -------------------------------------------
# Touch a few files in the fake sync root, list, and delete. The daemon
# is in offline dev mode so this exercises its in-process path-watching
# and IPC plumbing without hitting the network.
synthesise_activity() {
  local i
  local elapsed=0
  local interval=5
  while (( elapsed < RUN_DURATION_SECS )); do
    if ! kill -0 "${HEAPTRACK_PID}" 2>/dev/null; then
      return
    fi
    for i in 1 2 3 4 5; do
      printf 'memprofile-payload-%s-%s\n' "${elapsed}" "${i}" \
        >"${SYNC_ROOT}/file-${i}.txt"
    done
    ls "${SYNC_ROOT}" >/dev/null
    rm -f "${SYNC_ROOT}/file-1.txt"
    sleep "${interval}"
    elapsed=$(( elapsed + interval ))
  done
}
synthesise_activity &
ACTIVITY_PID=$!

# Wait the requested duration, then ask the daemon to exit cleanly.
sleep "${RUN_DURATION_SECS}"
if kill -0 "${HEAPTRACK_PID}" 2>/dev/null; then
  kill -TERM "${HEAPTRACK_PID}" 2>/dev/null || true
fi
wait "${ACTIVITY_PID}" 2>/dev/null || true
wait "${HEAPTRACK_PID}" 2>/dev/null || true

# --- Locate the actual capture file --------------------------------------
# heaptrack appends .gz / .zst depending on the build; pick the newest
# matching file in OUT_DIR.
ACTUAL_CAPTURE="$(ls -1t "${OUT_DIR}"/pcloudd.heaptrack* 2>/dev/null | head -n 1 || true)"
if [[ -z "${ACTUAL_CAPTURE}" || ! -s "${ACTUAL_CAPTURE}" ]]; then
  echo "[memprofile] heaptrack produced no capture file" >&2
  exit 3
fi
echo "[memprofile] capture file: ${ACTUAL_CAPTURE}"

# --- Extract JSON report -------------------------------------------------
if ! heaptrack_print --json "${ACTUAL_CAPTURE}" >"${JSON_REPORT}" 2>"${OUT_DIR}/heaptrack_print.stderr"; then
  echo "[memprofile] heaptrack_print --json failed; see ${OUT_DIR}/heaptrack_print.stderr" >&2
  exit 3
fi

# --- Pull the metrics we gate on -----------------------------------------
# heaptrack_print --json shape is loosely documented; we tolerate two
# common shapes (`peakRSS` / `totalAllocations` at top level OR nested
# under `summary`). If neither path yields a number, fail with exit 3.
PEAK_RSS_BYTES="$(jq -r '
  (.peakRSS // .summary.peakRSS // .totals.peakRSS // empty) | tonumber? // empty
' "${JSON_REPORT}")"
TOTAL_ALLOCS="$(jq -r '
  (.totalAllocations // .summary.totalAllocations // .totals.totalAllocations // empty) | tonumber? // empty
' "${JSON_REPORT}")"

if [[ -z "${PEAK_RSS_BYTES}" || -z "${TOTAL_ALLOCS}" ]]; then
  echo "[memprofile] could not extract peakRSS/totalAllocations from ${JSON_REPORT}" >&2
  echo "[memprofile] inspect the JSON manually: heaptrack_print --json may have changed shape." >&2
  exit 3
fi

echo "[memprofile] peak RSS:        ${PEAK_RSS_BYTES} bytes"
echo "[memprofile] total allocations: ${TOTAL_ALLOCS}"

# --- Update-baseline branch (operator-only) -----------------------------
if (( UPDATE_BASELINE == 1 )); then
  jq -n \
    --argjson peak_rss "${PEAK_RSS_BYTES}" \
    --argjson total_allocs "${TOTAL_ALLOCS}" \
    --arg run_secs "${RUN_DURATION_SECS}" \
    --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{
       schema: "pcloud-rs/memprofile/baseline-v1",
       peak_rss_bytes: $peak_rss,
       total_allocations: $total_allocs,
       run_duration_secs: ($run_secs | tonumber),
       recorded_at: $recorded_at
     }' >"${BASELINE_FILE}"
  echo "[memprofile] baseline updated at ${BASELINE_FILE}"
  exit 0
fi

# --- Gate (cold-start vs compare) ---------------------------------------
set +e
compare_against_baseline "${PEAK_RSS_BYTES}" "${TOTAL_ALLOCS}" "${BASELINE_FILE}"
gate_rc=$?
set -e
exit "${gate_rc}"
