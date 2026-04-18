#!/bin/bash
# Detect FUSE version and set appropriate flags.
#
# On macOS, the primary backend is fuse-t (no header files — detected by
# dylib presence). macFUSE is a secondary option detected by its framework
# bundle or headers. On Linux, pkg-config and header searches are used.

OS="$(uname -s)"

# ── macOS: probe for fuse-t or macFUSE dylib/bundle ──────────────────────────
if [ "$OS" = "Darwin" ]; then
    # fuse-t canonical dylib locations (Homebrew arm64, Homebrew x86_64, direct install)
    FUSET_CANDIDATES=(
        "/usr/local/lib/libfuse-t.dylib"
        "/opt/homebrew/lib/libfuse-t.dylib"
        "/Library/Application Support/fuse-t/lib/libfuse-t.dylib"
    )
    for dylib in "${FUSET_CANDIDATES[@]}"; do
        if [ -f "$dylib" ]; then
            echo "FUSE_T"
            exit 0
        fi
    done

    # macFUSE detection: kernel extension bundle (modern install)
    if [ -d "/Library/Filesystems/macfuse.fs" ] || [ -f "/usr/local/lib/libosxfuse.dylib" ]; then
        echo "MACFUSE"
        exit 0
    fi

    # macFUSE headers in Homebrew location
    for base in /usr/local/include /opt/homebrew/include; do
        if [ -f "$base/fuse/fuse.h" ] || [ -f "$base/fuse3/fuse.h" ]; then
            echo "MACFUSE"
            exit 0
        fi
    done

    echo "NONE"
    exit 1
fi

# ── Linux / BSD: pkg-config then header fallback ─────────────────────────────
if command -v pkg-config >/dev/null 2>&1; then
    if pkg-config --exists fuse3 2>/dev/null; then
        echo "FUSE3"
        exit 0
    elif pkg-config --exists fuse 2>/dev/null; then
        echo "FUSE2"
        exit 0
    fi
fi

SEARCH_PATHS="/usr/include /usr/local/include /opt/local/include /opt/include"

for base in $SEARCH_PATHS; do
    if [ -f "$base/fuse3/fuse.h" ]; then
        echo "FUSE3"
        exit 0
    fi
done

for base in $SEARCH_PATHS; do
    if [ -f "$base/fuse/fuse.h" ]; then
        echo "FUSE2"
        exit 0
    fi
done

# Last resort: try to compile a test program
if command -v gcc >/dev/null 2>&1; then
    if echo '#include <fuse.h>' | gcc -E -DFUSE_USE_VERSION=30 - >/dev/null 2>&1; then
        echo "FUSE3"
        exit 0
    fi
    if echo '#include <fuse.h>' | gcc -E -DFUSE_USE_VERSION=26 - >/dev/null 2>&1; then
        echo "FUSE2"
        exit 0
    fi
fi

echo "NONE"
exit 1
