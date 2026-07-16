#!/usr/bin/env bash
set -euo pipefail

# Run correctly from any working directory.  This is used by release hosts,
# where the checkout path and current directory are not guaranteed.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"
CARGO_MANIFEST="${REPO_ROOT}/Cargo.toml"
NFPM_MANIFEST="${REPO_ROOT}/packaging/debian/nfpm.yaml"

CARGO_VERSION="$({
  awk '
    /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
    /^\[/ { in_workspace_package = 0 }
    in_workspace_package && $1 == "version" {
      value = $3
      gsub(/^"|"$/, "", value)
      print value
      exit
    }
  ' "${CARGO_MANIFEST}"
})"
NFPM_VERSION="$(awk '$1 == "version:" { print $2; exit }' "${NFPM_MANIFEST}")"

if [ -z "${CARGO_VERSION}" ] || [ -z "${NFPM_VERSION}" ]; then
  printf 'ERROR: could not read workspace or nfpm version declaration\n' >&2
  exit 1
fi

case "${NFPM_VERSION}" in
  '${VERSION:-'*'}')
    # nfpm expands this expression at package time.  A development fallback
    # is intentionally not the release version, so validate the release input
    # when supplied and otherwise validate that the manifest remains templated.
    NFPM_DEFAULT="${NFPM_VERSION#'${VERSION:-'}"
    NFPM_DEFAULT="${NFPM_DEFAULT%'}'}"
    if [ -n "${VERSION:-}" ] && [ "${VERSION}" != "${CARGO_VERSION}" ]; then
      printf 'ERROR: Cargo.toml version (%s) != release VERSION (%s)\n' \
        "${CARGO_VERSION}" "${VERSION}" >&2
      exit 1
    fi
    if [ -n "${VERSION:-}" ]; then
      printf 'Versions match: %s (Cargo workspace and nfpm VERSION)\n' \
        "${CARGO_VERSION}"
    else
      printf 'Version template valid: workspace=%s, nfpm=VERSION (development default %s)\n' \
        "${CARGO_VERSION}" "${NFPM_DEFAULT}"
    fi
    ;;
  *)
    NFPM_VERSION="${NFPM_VERSION#\"}"
    NFPM_VERSION="${NFPM_VERSION%\"}"
    if [ "${CARGO_VERSION}" != "${NFPM_VERSION}" ]; then
      printf 'ERROR: Cargo.toml version (%s) != nfpm.yaml version (%s)\n' \
        "${CARGO_VERSION}" "${NFPM_VERSION}" >&2
      exit 1
    fi
    printf 'Versions match: %s\n' "${CARGO_VERSION}"
    ;;
esac
