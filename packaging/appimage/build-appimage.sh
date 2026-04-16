#!/usr/bin/env bash
# PLATFORM: Linux
#
# Build a portable AppImage for pcloud-rs (Rust rewrite).
#
# Prereqs (Debian/Ubuntu):
#   sudo apt-get install -y curl file libfuse2 libfuse3-dev pkg-config libsqlite3-dev
# Plus a Rust 1.82+ toolchain (`rustup` recommended).
#
# Usage:
#   packaging/appimage/build-appimage.sh [--arch x86_64]
#
# Output: ./pcloud-rs-<arch>.AppImage in the current working directory.

set -euo pipefail

ARCH="${ARCH:-x86_64}"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,20p' "$0"
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
RUST_DIR="${REPO_ROOT}/"
WORK_DIR="${REPO_ROOT}/build/appimage"
APPDIR="${WORK_DIR}/AppDir"
TOOL_DIR="${WORK_DIR}/tools"

echo "[appimage] repo root : ${REPO_ROOT}"
echo "[appimage] rust dir  : ${RUST_DIR}"
echo "[appimage] arch      : ${ARCH}"

rm -rf "${APPDIR}"
mkdir -p "${APPDIR}/usr/bin" \
         "${APPDIR}/usr/share/applications" \
         "${APPDIR}/usr/share/icons/hicolor/256x256/apps" \
         "${APPDIR}/usr/share/metainfo" \
         "${TOOL_DIR}"

# 1. Build release binaries.
echo "[appimage] building Rust workspace ..."
( cd "${RUST_DIR}" && cargo build --release --workspace --locked )

install -m 0755 "${RUST_DIR}/target/release/pcloudc" "${APPDIR}/usr/bin/pcloudc"
install -m 0755 "${RUST_DIR}/target/release/pcloudd" "${APPDIR}/usr/bin/pcloudd"

# 2. Desktop file + AppRun.
install -m 0644 "${SCRIPT_DIR}/pcloud-rs.desktop" \
                "${APPDIR}/usr/share/applications/pcloud-rs.desktop"
install -m 0644 "${SCRIPT_DIR}/pcloud-rs.desktop" "${APPDIR}/pcloud-rs.desktop"
install -m 0755 "${SCRIPT_DIR}/AppRun"          "${APPDIR}/AppRun"

# 3. Icon: use a repository icon if we can find one, otherwise a 1x1 PNG placeholder.
ICON_SRC=""
for cand in \
    "${REPO_ROOT}/packaging/pcloud-rs.png" \
    "${REPO_ROOT}/packaging/icons/pcloud-rs.png" \
    "${REPO_ROOT}/pcloud-rs.png"
do
    if [ -f "${cand}" ]; then ICON_SRC="${cand}"; break; fi
done

if [ -n "${ICON_SRC}" ]; then
    install -m 0644 "${ICON_SRC}" \
        "${APPDIR}/usr/share/icons/hicolor/256x256/apps/pcloud-rs.png"
    install -m 0644 "${ICON_SRC}" "${APPDIR}/pcloud-rs.png"
else
    # 1x1 transparent PNG placeholder (base64-encoded).
    PNG_B64='iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII='
    echo "${PNG_B64}" | base64 -d > "${APPDIR}/usr/share/icons/hicolor/256x256/apps/pcloud-rs.png"
    cp "${APPDIR}/usr/share/icons/hicolor/256x256/apps/pcloud-rs.png" "${APPDIR}/pcloud-rs.png"
    echo "[appimage] warning: no icon found, used placeholder" >&2
fi

# 4. Fetch appimagetool if we don't already have one on PATH.
APPIMAGETOOL="$(command -v appimagetool || true)"
if [ -z "${APPIMAGETOOL}" ]; then
    APPIMAGETOOL="${TOOL_DIR}/appimagetool-${ARCH}.AppImage"
    if [ ! -x "${APPIMAGETOOL}" ]; then
        URL="https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-${ARCH}.AppImage"
        echo "[appimage] downloading ${URL}"
        curl -fsSL -o "${APPIMAGETOOL}" "${URL}"
        chmod +x "${APPIMAGETOOL}"
    fi
fi

OUTPUT="${REPO_ROOT}/pcloud-rs-${ARCH}.AppImage"
echo "[appimage] assembling ${OUTPUT}"
ARCH="${ARCH}" "${APPIMAGETOOL}" "${APPDIR}" "${OUTPUT}"

echo
echo "[appimage] done: ${OUTPUT}"
echo "[appimage] manual verification:"
echo "    chmod +x ${OUTPUT}"
echo "    ${OUTPUT} --version"
