#!/bin/sh

set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "${script_dir}/../.." && pwd)
tmp=${TMPDIR:-/tmp}/pcloud-rs-unix-package-validation.$$
trap 'rm -rf "${tmp}"' EXIT HUP INT TERM
mkdir -p "${tmp}/first" "${tmp}/second" "${tmp}/extract"

for script in \
    "${script_dir}/build-tarball.sh" \
    "${script_dir}/validate.sh" \
    "${repo_root}/packaging/init/common/pcloudd-wrapper.sh" \
    "${repo_root}/packaging/freebsd/pcloudd.rc" \
    "${repo_root}/packaging/netbsd/pcloudd" \
    "${repo_root}/packaging/openbsd/pcloudd" \
    "${repo_root}/packaging/dragonfly/pcloudd" \
    "${repo_root}/packaging/init/freebsd/pcloudd" \
    "${repo_root}/packaging/init/netbsd/pcloudd" \
    "${repo_root}/packaging/init/openbsd/pcloudd" \
    "${repo_root}/packaging/solarish/pcloudd"; do
    sh -n "${script}"
done

if grep -R -n 'rc\.subr writes.*pidfile\|rc\.subr creates.*pidfile' \
    "${repo_root}/packaging/freebsd" \
    "${repo_root}/packaging/netbsd" \
    "${repo_root}/packaging/init"; then
    echo "BSD service assets must not claim that rc.subr creates daemon PID files" >&2
    exit 1
fi

if command -v xmllint >/dev/null 2>&1; then
    xmllint --noout "${repo_root}/packaging/solarish/pcloudd.xml"
fi

for platform in dragonfly omnios solaris; do
    SOURCE_DATE_EPOCH=1700000000 "${script_dir}/build-tarball.sh" \
        --platform "${platform}" \
        --version 0.1.0-test \
        --arch test-arch \
        --pcloudd /bin/true \
        --pcloudc /bin/true \
        --output "${tmp}/first" >/dev/null
    SOURCE_DATE_EPOCH=1700000000 "${script_dir}/build-tarball.sh" \
        --platform "${platform}" \
        --version 0.1.0-test \
        --arch test-arch \
        --pcloudd /bin/true \
        --pcloudc /bin/true \
        --output "${tmp}/second" >/dev/null

    archive="pcloud-rs-0.1.0-test-${platform}-test-arch.tar.gz"
    cmp "${tmp}/first/${archive}" "${tmp}/second/${archive}"
    (cd "${tmp}/first" && sha256sum -c "${archive}.sha256") >/dev/null

    rm -rf "${tmp}/extract/${platform}"
    mkdir -p "${tmp}/extract/${platform}"
    tar -xzf "${tmp}/first/${archive}" -C "${tmp}/extract/${platform}"
    stage="${tmp}/extract/${platform}/pcloud-rs-0.1.0-test-${platform}-test-arch"
    (cd "${stage}" && sha256sum -c MANIFEST.sha256) >/dev/null
    test -x "${stage}/root/usr/local/bin/pcloudd"
    test -x "${stage}/root/usr/local/bin/pcloudc"
    test -x "${stage}/root/usr/local/libexec/pcloudd-wrapper.sh"
    ! grep -R '@VERSION@\|@PLATFORM@\|@ARCH@' "${stage}/INSTALL.md"

    case "${platform}" in
        dragonfly)
            test -x "${stage}/root/usr/local/etc/rc.d/pcloudd"
            ;;
        omnios|solaris)
            test -x "${stage}/root/lib/svc/method/pcloudd"
            test -f "${stage}/root/lib/svc/manifest/site/pcloud-rs.xml"
            ;;
    esac
done

echo "portable Unix package validation passed"
