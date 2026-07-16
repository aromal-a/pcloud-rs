#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# diff-repro-builds.sh
#
# CI helper used by the macOS and Windows reproducible-build workflows
# (and reusable on Linux). Given two directories that each contain the
# `release-repro` outputs of a build run, hash every binary in each and
# fail loudly if the SHA-256 manifests do not match.
#
# This is intentionally a thin shell helper, not a Rust binary, so it
# runs identically on macOS-latest, windows-latest (under git-bash /
# msys2) and ubuntu-latest. The richer per-binary build driver lives at
# `packaging/scripts/verify-reproducibility.sh` and is Linux-only.
#
# Inputs:
#   $1 — first build output directory (e.g. .../target/release-repro)
#   $2 — second build output directory
#   $3 (optional) — space-separated list of binary basenames to compare.
#                   Defaults to "pcloudc pcloudd" on Linux/macOS and to
#                   "pcloudc.exe pcloudd.exe" on Windows.
#
# Exit codes:
#   0 — identical SHA-256 manifests (reproducibility OK).
#   1 — divergence detected.
#   2 — precondition missing (sha256sum not in PATH, or input dir absent).
#
# Notes:
#   - On macOS, GitHub-hosted runners ship `shasum -a 256` rather than
#     `sha256sum`. The script auto-detects which is available and
#     normalises output to two columns: `<hash>  <basename>`.
#   - On Windows under git-bash, `sha256sum` is provided by msys2; if
#     it is missing we fall back to `certutil -hashfile <file> SHA256`
#     and reformat. We do NOT require PowerShell.
# ------------------------------------------------------------------------------
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
usage: diff-repro-builds.sh <dir-a> <dir-b> [basenames...]

Compares SHA-256 of each binary basename across <dir-a> and <dir-b>.
Default basenames: "pcloudc pcloudd" (or *.exe on Windows).
USAGE
  exit 2
}

if [ "$#" -lt 2 ]; then
  usage
fi

DIR_A="$1"
DIR_B="$2"
shift 2

if [ ! -d "${DIR_A}" ]; then
  printf 'ERROR: not a directory: %s\n' "${DIR_A}" >&2
  exit 2
fi
if [ ! -d "${DIR_B}" ]; then
  printf 'ERROR: not a directory: %s\n' "${DIR_B}" >&2
  exit 2
fi

# Detect platform-default basenames if none were supplied.
if [ "$#" -eq 0 ]; then
  case "${OSTYPE:-}" in
    msys*|cygwin*|win32*) set -- pcloudc.exe pcloudd.exe ;;
    *)                    set -- pcloudc pcloudd ;;
  esac
fi

# Pick a hashing primitive that exists on this runner.
hash_cmd=""
if command -v sha256sum >/dev/null 2>&1; then
  hash_cmd="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  hash_cmd="shasum -a 256"
elif command -v certutil >/dev/null 2>&1; then
  hash_cmd="certutil"
else
  printf 'ERROR: no SHA-256 tool found (need sha256sum, shasum, or certutil)\n' >&2
  exit 2
fi

hash_one() {
  # Emit: "<hex>  <basename>"
  local file="$1"
  local base
  base="$(basename "${file}")"
  case "${hash_cmd}" in
    sha256sum)
      sha256sum "${file}" | awk -v b="${base}" '{print $1"  "b}'
      ;;
    "shasum -a 256")
      shasum -a 256 "${file}" | awk -v b="${base}" '{print $1"  "b}'
      ;;
    certutil)
      # certutil emits human-readable framing; pluck the bare hex line.
      local hex
      hex="$(certutil -hashfile "${file}" SHA256 \
        | tr -d '\r' \
        | awk 'NR==2{gsub(/ /,"");print tolower($0)}')"
      printf '%s  %s\n' "${hex}" "${base}"
      ;;
  esac
}

manifest_for_dir() {
  local dir="$1"
  shift
  for base in "$@"; do
    local f="${dir}/${base}"
    if [ ! -f "${f}" ]; then
      printf 'ERROR: missing binary: %s\n' "${f}" >&2
      exit 1
    fi
    hash_one "${f}"
  done
}

A_MANIFEST="$(manifest_for_dir "${DIR_A}" "$@")"
B_MANIFEST="$(manifest_for_dir "${DIR_B}" "$@")"

echo "[diff-repro] dir-a (${DIR_A}):"
echo "${A_MANIFEST}"
echo "[diff-repro] dir-b (${DIR_B}):"
echo "${B_MANIFEST}"

if [ "${A_MANIFEST}" = "${B_MANIFEST}" ]; then
  echo "[diff-repro] OK: byte-identical across both runs"
  exit 0
fi

echo "[diff-repro] FAIL: SHA-256 manifests differ between runs" >&2
diff -u <(printf '%s\n' "${A_MANIFEST}") <(printf '%s\n' "${B_MANIFEST}") || true
exit 1
