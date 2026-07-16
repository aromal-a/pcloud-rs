#!/bin/sh

set -eu

QPKG_NAME=pcloud-rs
QPKG_CONF=/etc/config/qpkg.conf
QPKG_DIR=$(getcfg "$QPKG_NAME" Install_Path -f "$QPKG_CONF")
[ -n "$QPKG_DIR" ] || { echo "cannot resolve $QPKG_NAME install path" >&2; exit 1; }

export QNAP_QPKG="$QPKG_NAME"
export PCLOUD_PACKAGE_DIR="$QPKG_DIR"
export PCLOUD_DATA_DIR="$QPKG_DIR/var"
export PCLOUD_RUN_DIR="$QPKG_DIR/var/run"
export PCLOUD_LOG_DIR="$QPKG_DIR/var/log"

exec "$QPKG_DIR/pcloudd-supervisor.sh" "${1:-}"
