#!/bin/sh

set -eu
umask 022

usage()
{
    cat >&2 <<'USAGE'
usage: build-tarball.sh --platform dragonfly|omnios|solaris --version VERSION
       --pcloudd PATH --pcloudc PATH [--arch ARCH] [--output DIR]

SOURCE_DATE_EPOCH controls normalized archive mtimes and defaults to 0.
GNU tar and gzip are required so owner, ordering, and timestamps are stable.
USAGE
    exit 2
}

platform=
version=
arch=
pcloudd=
pcloudc=
output=dist

while [ "$#" -gt 0 ]; do
    case "$1" in
        --platform) [ "$#" -ge 2 ] || usage; platform=$2; shift 2 ;;
        --version) [ "$#" -ge 2 ] || usage; version=$2; shift 2 ;;
        --arch) [ "$#" -ge 2 ] || usage; arch=$2; shift 2 ;;
        --pcloudd) [ "$#" -ge 2 ] || usage; pcloudd=$2; shift 2 ;;
        --pcloudc) [ "$#" -ge 2 ] || usage; pcloudc=$2; shift 2 ;;
        --output) [ "$#" -ge 2 ] || usage; output=$2; shift 2 ;;
        -h|--help) usage ;;
        *) echo "unknown argument: $1" >&2; usage ;;
    esac
done

case "${platform}" in
    dragonfly|omnios|solaris) ;;
    *) echo "unsupported or missing --platform: ${platform}" >&2; usage ;;
esac

[ -n "${version}" ] || usage
[ -x "${pcloudd}" ] || { echo "pcloudd is not executable: ${pcloudd}" >&2; exit 1; }
[ -x "${pcloudc}" ] || { echo "pcloudc is not executable: ${pcloudc}" >&2; exit 1; }

case "${version}" in
    *[!A-Za-z0-9._+-]*) echo "unsafe version component: ${version}" >&2; exit 1 ;;
esac

if [ -z "${arch}" ]; then
    arch=$(uname -m)
fi
case "${arch}" in
    *[!A-Za-z0-9._+-]*) echo "unsafe architecture component: ${arch}" >&2; exit 1 ;;
esac

epoch=${SOURCE_DATE_EPOCH:-0}
case "${epoch}" in
    ''|*[!0-9]*) echo "SOURCE_DATE_EPOCH must be an unsigned integer" >&2; exit 1 ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "${script_dir}/../.." && pwd)
mkdir -p "${output}"
output=$(CDPATH= cd -- "${output}" && pwd)

tar_bin=
for candidate in gtar /usr/gnu/bin/tar /opt/ooce/bin/gtar tar; do
    if command -v "${candidate}" >/dev/null 2>&1 && \
        "${candidate}" --version 2>/dev/null | grep -q 'GNU tar'; then
        tar_bin=${candidate}
        break
    fi
done
[ -n "${tar_bin}" ] || {
    echo "GNU tar is required for deterministic candidate archives" >&2
    exit 1
}
command -v gzip >/dev/null 2>&1 || { echo "gzip is required" >&2; exit 1; }

hash_file()
{
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v sha256 >/dev/null 2>&1; then
        sha256 -q "$1"
    elif command -v digest >/dev/null 2>&1; then
        digest -a sha256 "$1"
    else
        echo "no SHA-256 utility found (sha256sum, sha256, or digest)" >&2
        return 1
    fi
}

base="pcloud-rs-${version}-${platform}-${arch}"
tmp=${TMPDIR:-/tmp}/pcloud-rs-unix-package.$$
trap 'rm -rf "${tmp}"' EXIT HUP INT TERM
stage="${tmp}/${base}"
root="${stage}/root"

mkdir -p \
    "${root}/usr/local/bin" \
    "${root}/usr/local/libexec" \
    "${root}/usr/local/share/doc/pcloud-rs" \
    "${root}/usr/local/share/man/man1"

install -m 0555 "${pcloudd}" "${root}/usr/local/bin/pcloudd"
install -m 0555 "${pcloudc}" "${root}/usr/local/bin/pcloudc"
install -m 0555 "${repo_root}/packaging/init/common/pcloudd-wrapper.sh" \
    "${root}/usr/local/libexec/pcloudd-wrapper.sh"
install -m 0444 "${repo_root}/packaging/man/pcloudd.1" \
    "${root}/usr/local/share/man/man1/pcloudd.1"
install -m 0444 "${repo_root}/packaging/man/pcloudc.1" \
    "${root}/usr/local/share/man/man1/pcloudc.1"
install -m 0444 "${repo_root}/LICENSE-APACHE" "${stage}/LICENSE-APACHE"
install -m 0444 "${repo_root}/LICENSE-MIT" "${stage}/LICENSE-MIT"

sed \
    -e "s/@VERSION@/${version}/g" \
    -e "s/@PLATFORM@/${platform}/g" \
    -e "s/@ARCH@/${arch}/g" \
    "${script_dir}/INSTALL.md.in" >"${stage}/INSTALL.md"

case "${platform}" in
    dragonfly)
        mkdir -p "${root}/usr/local/etc/pcloud-rs" \
            "${root}/usr/local/etc/rc.d"
        install -m 0400 "${repo_root}/packaging/init/common/pcloudd.env.example" \
            "${root}/usr/local/etc/pcloud-rs/pcloudd.env.example"
        install -m 0555 "${repo_root}/packaging/dragonfly/pcloudd" \
            "${root}/usr/local/etc/rc.d/pcloudd"
        install -m 0444 "${repo_root}/packaging/dragonfly/README.md" \
            "${root}/usr/local/share/doc/pcloud-rs/PLATFORM.md"
        ;;
    omnios|solaris)
        mkdir -p "${root}/etc/pcloud-rs" \
            "${root}/lib/svc/manifest/site" \
            "${root}/lib/svc/method"
        install -m 0400 "${repo_root}/packaging/init/common/pcloudd.env.example" \
            "${root}/etc/pcloud-rs/pcloudd.env.example"
        install -m 0444 "${repo_root}/packaging/solarish/pcloudd.xml" \
            "${root}/lib/svc/manifest/site/pcloud-rs.xml"
        install -m 0555 "${repo_root}/packaging/solarish/pcloudd" \
            "${root}/lib/svc/method/pcloudd"
        install -m 0444 "${repo_root}/packaging/solarish/README.md" \
            "${root}/usr/local/share/doc/pcloud-rs/PLATFORM.md"
        ;;
esac

(
    cd "${stage}"
    find . -type f ! -name MANIFEST.sha256 -print | LC_ALL=C sort |
        while IFS= read -r file; do
            printf '%s  %s\n' "$(hash_file "${file}")" "${file#./}"
        done >MANIFEST.sha256
)

archive="${output}/${base}.tar.gz"
archive_tmp="${tmp}/${base}.tar.gz"
(
    cd "${tmp}"
    "${tar_bin}" \
        --sort=name \
        --mtime="@${epoch}" \
        --owner=0 \
        --group=0 \
        --numeric-owner \
        --format=ustar \
        -cf - "${base}" | gzip -n -9 >"${archive_tmp}"
)
mv "${archive_tmp}" "${archive}"
printf '%s  %s\n' "$(hash_file "${archive}")" "$(basename "${archive}")" \
    >"${archive}.sha256"

printf '%s\n' "${archive}"
