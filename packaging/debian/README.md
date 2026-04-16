# Debian / nfpm packaging

This directory contains `.deb` (and cross-distro RPM) packaging assets
for the Rust rewrite of `pcloud-rs`.

## Recommended: nfpm

[nfpm](https://nfpm.goreleaser.com/) can build `.deb`, `.rpm`, `.apk`,
and `.ipk` packages from a single YAML descriptor without Debian tooling.

```sh
# 1. Build release binaries from :
cd . && cargo build --release --workspace

# 2. Validate the nfpm config:
nfpm --config packaging/debian/nfpm.yaml check

# 3. Build a .deb:
mkdir -p dist
nfpm pkg --config packaging/debian/nfpm.yaml \
         --packager deb --target dist/

# 4. (Optional) RPM too:
nfpm pkg --config packaging/debian/nfpm.yaml \
         --packager rpm --target dist/
```

## Alternative: cargo-deb

See `cargo-deb.toml` in this directory for a ready-to-paste
`[package.metadata.deb]` snippet. It is not wired into the crate
`Cargo.toml` files by default to keep source untouched.

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
