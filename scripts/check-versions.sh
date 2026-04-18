#!/usr/bin/env bash
set -euo pipefail
CARGO_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
NFPM_VERSION=$(grep '^version:' packaging/debian/nfpm.yaml | sed 's/version: "\(.*\)"/\1/')
if [ "$CARGO_VERSION" != "$NFPM_VERSION" ]; then
  echo "ERROR: Cargo.toml version ($CARGO_VERSION) != nfpm.yaml version ($NFPM_VERSION)" >&2
  exit 1
fi
echo "Versions match: $CARGO_VERSION"
