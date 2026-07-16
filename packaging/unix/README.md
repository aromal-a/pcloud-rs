# Portable Unix package candidates

`build-tarball.sh` assembles native `pcloudc` and `pcloudd` binaries with the
service definition, environment template, man pages, licenses, and per-file
SHA-256 manifest for:

- DragonFly BSD (`rc.d`);
- OmniOS/illumos (SMF);
- Oracle Solaris (SMF).

The archive is deterministic for identical input binaries and
`SOURCE_DATE_EPOCH`: GNU tar fixes entry order, ownership, format, and mtimes,
while `gzip -n` removes the gzip header timestamp. Example:

```sh
cargo build --release --locked -p pcloud-daemon -p pcloud-cli
SOURCE_DATE_EPOCH="$(git log -1 --pretty=%ct)" \
  packaging/unix/build-tarball.sh \
    --platform dragonfly \
    --version 0.1.0 \
    --pcloudd target/release/pcloudd \
    --pcloudc target/release/pcloudc
```

These are auditable package candidates rather than published operating-system
packages. They do not create users, write credentials, enable services, or
claim that a native install/upgrade test passed. Run `validate.sh` locally;
native CI additionally validates the rc.d/SMF asset and packages binaries
built on the target OS.
