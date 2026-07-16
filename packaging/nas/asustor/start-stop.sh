#!/bin/sh

set -eu

PACKAGE_DIR="${APKG_PKG_DIR:-/usr/local/AppCentral/pcloud-rs}"
export PCLOUD_PACKAGE_DIR="$PACKAGE_DIR"
export PCLOUD_DATA_DIR="$PACKAGE_DIR/var"
export PCLOUD_RUN_DIR="$PACKAGE_DIR/var/run"
export PCLOUD_LOG_DIR="$PACKAGE_DIR/var/log"

exec "$PACKAGE_DIR/share/pcloud-rs/pcloudd-supervisor.sh" "${1:-}"
