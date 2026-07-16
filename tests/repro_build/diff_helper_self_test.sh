#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# diff_helper_self_test.sh
#
# Synthetic, no-toolchain self-test for `scripts/diff-repro-builds.sh`. The
# real macOS / Windows two-runner reproducible-build acceptance requires
# cross-OS GitHub Actions runners we do not control in the AI scope; this
# self-test instead validates that the helper script *itself* behaves
# correctly given controlled binary fixtures, so future edits to the helper
# cannot silently break its identical/divergent contract.
#
# Behaviour under test:
#   - Two directories with byte-identical "binaries" (`pcloudc`, `pcloudd`)
#     => helper exits 0 and prints "byte-identical".
#   - One byte mutated in `fixture-b/pcloudc`
#     => helper exits 1 and prints "SHA-256 manifests differ".
#
# Dependencies (intentional minimum):
#   - bash, dd, cp, mktemp, printf, head
#   - sha256sum or shasum -a 256 (the helper picks whichever exists; the
#     self-test does not call them directly)
#
# Exit codes:
#   0 — both fixtures produced the expected helper exit code; [PASS] printed.
#   1 — at least one assertion failed; [FAIL] printed with detail.
# ------------------------------------------------------------------------------
set -u
# NOTE: deliberately not using `set -e` — we *want* to capture non-zero exits
# from the helper without aborting the harness.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
HELPER="${REPO_ROOT}/scripts/diff-repro-builds.sh"

if [ ! -x "${HELPER}" ] && [ ! -f "${HELPER}" ]; then
  printf '[FAIL] helper not found at %s\n' "${HELPER}" >&2
  exit 1
fi

WORKDIR="$(mktemp -d -t pcloud-repro-self-test.XXXXXX)"
trap 'rm -rf "${WORKDIR}"' EXIT

FIX_A="${WORKDIR}/fixture-a"
FIX_B="${WORKDIR}/fixture-b"
mkdir -p "${FIX_A}" "${FIX_B}"

# Synthesize two "release" binaries in fixture-a. 1 KiB of urandom is enough
# to make collisions astronomically unlikely while keeping the test cheap.
dd if=/dev/urandom of="${FIX_A}/pcloudc" bs=1024 count=1 status=none
dd if=/dev/urandom of="${FIX_A}/pcloudd" bs=1024 count=1 status=none

# Mirror them byte-for-byte into fixture-b.
cp "${FIX_A}/pcloudc" "${FIX_B}/pcloudc"
cp "${FIX_A}/pcloudd" "${FIX_B}/pcloudd"

# --- Assertion 1: identical fixtures => helper exits 0. -----------------------
LOG_IDENTICAL="${WORKDIR}/identical.log"
bash "${HELPER}" "${FIX_A}" "${FIX_B}" >"${LOG_IDENTICAL}" 2>&1
RC_IDENTICAL=$?

# --- Assertion 2: mutate one byte of fixture-b/pcloudc => helper exits 1. ----
# Flip byte 0 by writing a single byte that is guaranteed to differ from the
# original. We read the original first byte, XOR with 0xFF, then seek-write.
ORIG_BYTE_HEX="$(head -c 1 "${FIX_B}/pcloudc" | od -An -tx1 | tr -d ' \n')"
ORIG_BYTE_DEC=$((16#${ORIG_BYTE_HEX}))
NEW_BYTE_DEC=$((ORIG_BYTE_DEC ^ 255))
# `printf '\xNN'` is portable enough for bash; pipe to dd seeking to byte 0.
printf '%b' "$(printf '\\x%02x' "${NEW_BYTE_DEC}")" \
  | dd of="${FIX_B}/pcloudc" bs=1 count=1 conv=notrunc status=none

LOG_DIVERGENT="${WORKDIR}/divergent.log"
bash "${HELPER}" "${FIX_A}" "${FIX_B}" >"${LOG_DIVERGENT}" 2>&1
RC_DIVERGENT=$?

# --- Report -------------------------------------------------------------------
printf '\n--- self-test summary ---\n'
printf 'identical run: exit=%d (expected 0)\n'   "${RC_IDENTICAL}"
printf 'divergent run: exit=%d (expected 1)\n'   "${RC_DIVERGENT}"

FAILED=0
if [ "${RC_IDENTICAL}" -ne 0 ]; then
  printf '  [assertion-1] FAILED — helper rejected identical fixtures\n' >&2
  printf '  --- identical.log ---\n' >&2
  sed 's/^/  | /' "${LOG_IDENTICAL}" >&2
  FAILED=1
fi
if [ "${RC_DIVERGENT}" -ne 1 ]; then
  printf '  [assertion-2] FAILED — helper did not detect mutated byte\n' >&2
  printf '  --- divergent.log ---\n' >&2
  sed 's/^/  | /' "${LOG_DIVERGENT}" >&2
  FAILED=1
fi

if [ "${FAILED}" -eq 0 ]; then
  printf '[PASS] diff-repro-builds.sh self-test (identical=>0, mutated=>1)\n'
  exit 0
fi
printf '[FAIL] diff-repro-builds.sh self-test\n' >&2
exit 1
