#!/bin/sh
set -eu
umask 077

: "${PCLOUDRS_DAEMON_BIN:=/usr/local/bin/pcloudd}"

if [ ! -x "${PCLOUDRS_DAEMON_BIN}" ]; then
  echo "pcloudd-wrapper: daemon binary not executable: ${PCLOUDRS_DAEMON_BIN}" >&2
  exit 1
fi

check_secret_file() {
  file="$1"
  label="$2"
  if [ ! -f "${file}" ]; then
    echo "pcloudd-wrapper: ${label} file does not exist: ${file}" >&2
    exit 1
  fi
  if [ ! -r "${file}" ]; then
    echo "pcloudd-wrapper: ${label} file is not readable: ${file}" >&2
    exit 1
  fi
}

token_file="${PCLOUDRS_TOKEN_FILE:-}"
username_file="${PCLOUDRS_USERNAME_FILE:-}"
password_file="${PCLOUDRS_PASSWORD_FILE:-}"
tfa_file="${PCLOUDRS_TFA_CODE_FILE:-}"
recovery_file="${PCLOUDRS_RECOVERY_CODE_FILE:-}"

if [ -n "${token_file}" ]; then
  check_secret_file "${token_file}" "token"
else
  if [ -n "${username_file}" ] || [ -n "${password_file}" ]; then
    if [ -z "${username_file}" ] || [ -z "${password_file}" ]; then
      echo "pcloudd-wrapper: both PCLOUDRS_USERNAME_FILE and PCLOUDRS_PASSWORD_FILE are required together" >&2
      exit 1
    fi
    check_secret_file "${username_file}" "username"
    check_secret_file "${password_file}" "password"
    if [ -n "${tfa_file}" ] && [ -n "${recovery_file}" ]; then
      echo "pcloudd-wrapper: set either PCLOUDRS_TFA_CODE_FILE or PCLOUDRS_RECOVERY_CODE_FILE, not both" >&2
      exit 1
    fi
    if [ -n "${tfa_file}" ]; then
      check_secret_file "${tfa_file}" "two-factor code"
    fi
    if [ -n "${recovery_file}" ]; then
      check_secret_file "${recovery_file}" "recovery code"
    fi
  fi
fi

exec "${PCLOUDRS_DAEMON_BIN}" serve
