#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# verify-reproducibility.sh
#
# Builds `pcloudc` and `pcloudd` twice with `cargo auditable` and the
# `release-repro` profile, then verifies that both builds produce
# byte-identical binaries (SHA-256 match).
#
# Rationale and pinning contract:
#   docs/book/src/development/reproducible-builds.md
#
# Pinning strategy:
#   - SOURCE_DATE_EPOCH=0 (fixed) so we do not depend on git state.
#     CI release pipelines override this with the tag commit time.
#   - --locked blocks dependency re-resolution between the two builds.
#   - cargo-auditable matches the release workflow's auditable binary format.
#   - --profile release-repro engages the deterministic profile pinned in
#     Cargo.toml (strip=symbols, debug=false, codegen-units=1,
#     lto=true, panic=abort).
#   - RUSTFLAGS: --remap-path-prefix scrubs the absolute checkout path and
#                -Wl,--build-id=none neutralises the ELF build-id.
#
# Exit codes:
#   0  — both binaries byte-identical for pcloudc and pcloudd.
#   1  — hash mismatch (reproducibility broken).
#   2  — precondition missing (toolchain, sha256sum, workspace not found).
#
# Usage:
#   packaging/scripts/verify-reproducibility.sh            # full two-build run
#   KEEP_ARTEFACTS=1 packaging/scripts/verify-reproducibility.sh
#       # preserves the per-build binary copies under /tmp/repro-verify-* for
#       # offline diffoscope analysis.
# ------------------------------------------------------------------------------
set -euo pipefail

# Resolve the repository root from this script's location so the script works
# whether invoked via absolute path, relative path, or symlink.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." >/dev/null 2>&1 && pwd)"
WORKSPACE="${REPO_ROOT}/"

if [ ! -f "${WORKSPACE}/Cargo.toml" ]; then
  printf 'ERROR:  workspace not found at %s\n' "${WORKSPACE}" >&2
  exit 2
fi

for tool in cargo cargo-auditable sha256sum; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    printf 'ERROR: required tool not in PATH: %s\n' "${tool}" >&2
    exit 2
  fi
done

# --- Deterministic build environment --------------------------------------
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
export LC_ALL=C
export TZ=UTC

# Scrub the absolute checkout path and pin the ELF build-id. The remap target
# is empty so panic/backtrace strings embed only relative paths. Extra entries
# scrub the cargo/rustup prefixes.
#
# NB: CARGO_HOME and HOME fall back to defaults when unset; the intent is that
# any absolute path that leaks into debug strings gets remapped to a stable
# placeholder.
export RUSTFLAGS="${RUSTFLAGS:-} \
--remap-path-prefix=${REPO_ROOT}= \
--remap-path-prefix=${CARGO_HOME:-${HOME}/.cargo}=/cargo \
--remap-path-prefix=${HOME}/.rustup=/rustup \
-C link-arg=-Wl,--build-id=none"

PROFILE="release-repro"
TARGET_DIR="${WORKSPACE}/target"

log() { printf '[verify-repro] %s\n' "$*"; }

build_once() {
  local label="$1"
  log "build ${label}: cargo auditable build --locked --profile ${PROFILE} -p pcloud-cli -p pcloud-daemon"
  (
    cd "${WORKSPACE}"
    cargo clean --profile "${PROFILE}" --quiet 2>/dev/null || true
    CARGO_PROFILE_RELEASE_REPRO_ACTIVE=1 \
      cargo auditable build \
      --locked \
      --profile "${PROFILE}" \
      -p pcloud-cli --bin pcloudc \
      -p pcloud-daemon --bin pcloudd
  )
}

snapshot_binaries() {
  # Copy the produced binaries to a label-specific directory so we can hash and
  # compare them even after the second build overwrites target/.
  local label="$1"
  local dest="$2"
  mkdir -p "${dest}"
  cp -p "${TARGET_DIR}/${PROFILE}/pcloudc" "${dest}/pcloudc"
  cp -p "${TARGET_DIR}/${PROFILE}/pcloudd" "${dest}/pcloudd"
  (cd "${dest}" && sha256sum pcloudc pcloudd > SHA256SUMS)
  log "snapshot ${label} -> ${dest}"
  cat "${dest}/SHA256SUMS"
}

WORKDIR="$(mktemp -d -t pcloud-rs-repro-XXXXXX)"
trap 'if [ -z "${KEEP_ARTEFACTS:-}" ]; then rm -rf "${WORKDIR}"; else log "artefacts preserved at ${WORKDIR}"; fi' EXIT

FIRST_DIR="${WORKDIR}/first"
SECOND_DIR="${WORKDIR}/second"

log "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}"
log "RUSTFLAGS=${RUSTFLAGS}"
log "workspace=${WORKSPACE}"
log "profile=${PROFILE}"

build_once "first"
snapshot_binaries "first" "${FIRST_DIR}"

build_once "second"
snapshot_binaries "second" "${SECOND_DIR}"

log "comparing SHA-256 manifests"
if diff -u "${FIRST_DIR}/SHA256SUMS" "${SECOND_DIR}/SHA256SUMS"; then
  log "OK: pcloudc and pcloudd are byte-identical across two consecutive builds"
  exit 0
fi

log "FAIL: byte-for-byte reproducibility check failed"
log "first:"
cat "${FIRST_DIR}/SHA256SUMS"
log "second:"
cat "${SECOND_DIR}/SHA256SUMS"
log "run 'diffoscope ${FIRST_DIR} ${SECOND_DIR}' to localise divergence"
exit 1
