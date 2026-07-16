# Debian / nfpm packaging

This directory contains `.deb` (and cross-distro RPM) packaging assets
for the Rust rewrite of `pcloud-rs`.

## Optional: nfpm

[nfpm](https://nfpm.goreleaser.com/) can build `.deb`, `.rpm`, `.apk`,
and `.ipk` packages from a single YAML descriptor without Debian tooling.
The public release workflow uses `cargo-deb` and `cargo-generate-rpm`; this
descriptor is a manually validated alternative and is not release evidence by
itself.

```sh
# 1. Build release binaries from :
cd . && cargo build --release --workspace

# 2. Validate the workspace/package version contract and the nfpm config:
scripts/check-versions.sh
VERSION=0.1.0 nfpm --config packaging/debian/nfpm.yaml check

# 3. Build a .deb:
mkdir -p dist
VERSION=0.1.0 nfpm pkg --config packaging/debian/nfpm.yaml \
         --packager deb --target dist/

# 4. (Optional) RPM too:
VERSION=0.1.0 nfpm pkg --config packaging/debian/nfpm.yaml \
         --packager rpm --target dist/
```

## Public release path: cargo-deb and cargo-generate-rpm

The active metadata lives in `crates/pcloud-daemon/Cargo.toml`; see
`.github/workflows/release-packaging.yml` for the authoritative build and
qualification commands. `cargo-deb.toml` is retained as reference material.

## Files

| File             | Purpose                                                  |
|------------------|----------------------------------------------------------|
| `nfpm.yaml`      | nfpm package descriptor (preferred path).                |
| `control`        | Reference Debian `control` stanza.                       |
| `postinst`       | Post-install maintainer script (systemd reload).         |
| `postrm`         | Post-remove maintainer script (systemd reload).          |
| `cargo-deb.toml` | Optional cargo-deb metadata snippet (not auto-loaded).   |

The maintainer scripts intentionally do NOT enable or start
`pcloudd.service` — users must opt in. They also do NOT remove user
state on purge; `$HOME/.config/pcloud-rs` and
`$HOME/.local/share/pcloud-rs` are the user's responsibility.
