# Reproducible Builds

> **Status honesty:** **pre-alpha**. No release tag exists yet; the CI
> double-build job and the published `SHA256SUMS` described below are the
> *contracts* we hold ourselves to for the first tag, not empirical artefacts
> downstream auditors can already verify. Treat this page as a binding
> specification that the release-cutting PR will exercise end-to-end.

## 1. Purpose

A reproducible build is one where any party — given the same source tree,
the same pinned toolchain, and the same dependency graph — produces a
**byte-identical** binary. Audience:

- **packagers** (Debian, Fedora, Homebrew, winget) who need to independently
  verify that the artefact on our GitHub Releases matches the source tag;
- **security teams** who demand supply-chain attestations before approving
  an internal deployment;
- **release managers** running the release-checklist gauntlet.

After reading this page you will know:

- what we pin to achieve byte-identical output,
- what we *cannot* pin (signed installer outer layers) and why,
- the exact verification procedure a third party runs to confirm our
  `SHA256SUMS` file matches a locally rebuilt binary.

## 2. Prerequisites

### Toolchain pin

- `rust-toolchain.toml` — `channel = "stable"`, components `clippy,
  rustfmt`. Rebuilders must install the exact same toolchain:

  ```sh
  rustup show          # prints the resolved toolchain
  ```

- Workspace: `edition = "2024"`, `rust-version = "1.85"`.
- Build profile: `--profile release-repro` (defined in `Cargo.toml`,
  inherits from `release-dist`; `strip = "debuginfo"`, `debug = 0`,
  `codegen-units = 1`). The core rationale block is at `Cargo.toml:91`.

### System dependencies

- `git` (to recover the tag commit time for `SOURCE_DATE_EPOCH`).
- `gpg` (to verify `SHA256SUMS.asc`).
- `cargo-vendor` — only for the fully-offline rebuild path (§4.4).
- `diffoscope` — only when hunting a reproducibility breakage.

### Locked environment

- `Cargo.lock` is **committed**.
- Do **not** `cargo update` on a release branch.
- Build inside the release container image (Debian stable + pinned
  toolchain) when targeting Linux releases — glibc, `ld`, and archive
  utilities must match.

## 3. Conceptual preamble — where reproducibility sits

```
 source tree (tagged)
        │
        ▼
 pinned toolchain (rust-toolchain.toml)
        │
        ▼
 locked deps (Cargo.lock + vendor/ optional)
        │
        ▼
 pinned env: SOURCE_DATE_EPOCH, --remap-path-prefix, --build-id=…
        │
        ▼
 cargo build --profile release-repro --locked
        │
        ▼
 byte-identical binary
```

Each arrow is one of the hazards the mitigations in §4 address.

### Non-goals (explicit)

- Reproducibility across **different toolchain versions** — a `rustup
  update` breaks the contract by design.
- Reproducibility across **different `glibc` / `libc++` versions** — those
  are separate artefacts.
- Reproducibility of **signed installer outer layers** (macOS `.pkg`,
  Windows `.msi`) — signatures and timestamp-server responses are
  intrinsically unique. The unsigned binary **inside** the installer is
  reproducible.
- Reproducibility of the **AppImage runtime stub** — shipped as-is from
  upstream AppImageKit. Only the squashfs payload is under our control.

## 4. Detailed walkthrough

### 4.1 What breaks reproducibility

Six sources of non-determinism matter for this tree:

1. **Embedded build timestamps.** `mtime` of input files leaks into tar
   archives and some codegen paths unless `SOURCE_DATE_EPOCH` is set.
2. **Absolute build paths.** By default, rustc embeds the full checkout
   path (e.g. `/home/runner/work/pcloud-rs/pcloud-rs/crates/…`) into
   debuginfo and some panic strings.
3. **ELF build-id.** `ld` defaults to `--build-id=sha1` (deterministic for
   the same inputs) or `--build-id=uuid` (random per invocation). We pin
   it explicitly.
4. **Non-deterministic codegen.** `codegen-units > 1` lets the compiler
   shuffle symbol order between runs. `release-repro` pins
   `codegen-units = 1`.
5. **Unlocked dependencies.** Cargo resolving a newer semver-compatible
   version between runs. `--locked` blocks this.
6. **Registry state.** `crates.io` cache mutations across runs. The
   vendored-deps recipe (§4.4) removes this dependency entirely.

### 4.2 `release-repro` profile

Defined in `Cargo.toml` starting around line **91**. Inherits from
`release-dist`:

```toml
[profile.release-repro]
inherits = "release-dist"
# keep symbols, drop DWARF — incident responders still get stack traces.
strip = "debuginfo"
debug = 0
# deterministic symbol layout.
codegen-units = 1
```

The inherited `release-dist` already sets `panic = "abort"`, `lto = "fat"`,
and `codegen-units = 1`. Do not override without updating this page.

### 4.3 Pinned inputs

`SOURCE_DATE_EPOCH` — pinned to the **tag commit** time:

```sh
export SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct)
```

`rustc` honours this; so do `tar`, `ar`, patched `zip`, `dpkg-deb`, and
`rpmbuild`. Without it, tarballs embed the current wall-clock time.

Path scrubbing — add these to `RUSTFLAGS` (or `.cargo/config.toml`):

```text
--remap-path-prefix=/home/runner/work/pcloud-rs/pcloud-rs=
--remap-path-prefix=$CARGO_HOME=/cargo
--remap-path-prefix=$HOME/.rustup=/rustup
```

The first entry matches the GitHub Actions Linux runner layout. Local
rebuilders either mirror the same absolute path **or** pass an equivalent
`--remap-path-prefix` so the embedded strings match.

Build-id — pin explicitly:

```text
-C link-arg=-Wl,--build-id=none
```

If a distro policy (e.g. Fedora debuginfod) requires a build-id, use
`--build-id=sha1` — SHA-1 over final content is deterministic. `uuid` is
not.

`--locked` — mandatory on every `cargo build` invocation during a release
so the lockfile cannot silently re-resolve.

`rust-toolchain.toml` — commit the exact channel. Rebuilders install:

```sh
rustup install stable                 # resolves to the pinned version
rustup component add clippy rustfmt
```

### 4.4 Vendored deps (optional, fully offline)

For rebuilders on air-gapped systems:

```sh
cargo vendor --locked vendor/ > .cargo/vendor-config.toml
```

Then in `.cargo/config.toml`:

```toml
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
```

Commit `vendor/` on the release branch (it is large; keep it off `main`).
This makes the build independent of `crates.io` availability and registry
index state.

### 4.5 Frozen flake (Nix rebuilders)

For Nix users, the repo root carries a `flake.nix`; see `flake.lock`.
Rebuilders run:

```sh
nix build --refresh .#pcloud-rs --no-write-lock-file
```

The lock file fully pins every input including nixpkgs revision, so the
build graph is reproducible end-to-end.

## 5. Verification procedure

### 5.1 One-shot script (recommended)

The repo ships a wrapper at
`packaging/scripts/verify-reproducibility.sh` that performs steps 1–5 below
for `pcloudc` and `pcloudd` together. This is the same script the
`reproducibility` CI job executes, so local success is a near-perfect
predictor of CI success.

```sh
# From the repo root:
packaging/scripts/verify-reproducibility.sh

# Keep the per-build binaries + manifests under /tmp for diffoscope analysis:
KEEP_ARTEFACTS=1 packaging/scripts/verify-reproducibility.sh
```

The script:

- exports `SOURCE_DATE_EPOCH=0` by default (override via the environment to
  match a tag commit time),
- exports a `RUSTFLAGS` that carries `--remap-path-prefix` for the checkout
  root, `$CARGO_HOME`, and `$HOME/.rustup`, plus
  `-C link-arg=-Wl,--build-id=none`,
- runs `cargo build --locked --profile release-repro -p pcloud-cli -p
  pcloud-daemon` twice (first build, `cargo clean --profile release-repro`,
  second build),
- snapshots each build's `pcloudc` / `pcloudd` under a temp directory and
  compares SHA-256 manifests,
- exits 0 only if both binaries match byte-for-byte across the two runs.

### 5.2 Manual procedure

The release job runs the equivalent double-build check on every release
tag. A third party may reproduce it by hand as follows:

```sh
git checkout v<version>

# 1. Pin build-time metadata.
export SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct)
export RUSTFLAGS='--remap-path-prefix=$PWD= -C link-arg=-Wl,--build-id=none'

# 2. Confirm toolchain pin.
rustc --version    # must match rust-toolchain.toml exactly

# 3. Build.
cargo build --profile release-repro --locked -p pcloud-cli -p pcloud-daemon
sha256sum target/release-repro/pcloudc target/release-repro/pcloudd > /tmp/first.sha

# 4. Scrub and rebuild.
cargo clean --profile release-repro
cargo build --profile release-repro --locked -p pcloud-cli -p pcloud-daemon
sha256sum target/release-repro/pcloudc target/release-repro/pcloudd > /tmp/second.sha

# 5. Compare.
diff /tmp/first.sha /tmp/second.sha       # must be empty
```

Then compare against the signed manifest:

```sh
# 6. Verify signature and local hash.
gpg --verify SHA256SUMS.asc SHA256SUMS
grep pcloud-cli SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
```

If the local hash matches the signed manifest, the release artefact is
verified end-to-end. If it does not, capture the divergence with
`diffoscope` and open a bead tagged `repro-regression`.

## 6. Gate checklist

On the release branch:

- [ ] `rust-toolchain.toml` pin unchanged since the previous release.
- [ ] `Cargo.lock` committed on the release tag.
- [ ] `Cargo.toml` carries `[profile.release-repro]` with
      `codegen-units = 1`, `strip = "debuginfo"`, `debug = 0`.
- [ ] `SOURCE_DATE_EPOCH` exported from the tag commit time in CI.
- [ ] `RUSTFLAGS` carries `--remap-path-prefix` for the runner root and
      `$CARGO_HOME` / `$HOME/.rustup`.
- [ ] `-C link-arg=-Wl,--build-id=none` (or `=sha1` per distro policy).
- [ ] Double-build job green: two consecutive builds produce identical
      SHA-256.
- [ ] `SHA256SUMS.asc` signed with the release GPG key.
- [ ] Verification procedure in §5 executed on a **clean VM** before the
      release is promoted from pre-release to "Latest".

## 7. Common mistakes

- **Forgot `SOURCE_DATE_EPOCH`.** *How reviewers catch it:* `diffoscope`
  reports tar-member `mtime` differences; the double-build hash diff fails.
- **Left the default build-id.** *How reviewers catch it:* `readelf -n`
  shows a different build-id between runs; double-build hashes differ.
- **Used `cargo build` without `--locked`.** *How reviewers catch it:*
  `Cargo.lock` diff surfaces in CI or the hash mismatches a fresh-clone
  rebuild.
- **Checked out to `/some/other/path` without a matching remap.** *How
  reviewers catch it:* absolute paths leak into `.note.gnu.build-id` /
  panic strings; `strings pcloud-cli | grep /home` finds them.
- **Mixed toolchains.** *How reviewers catch it:* `rustc --version` on the
  rebuild host does not match `rust-toolchain.toml`.
- **Ran `cargo update` on the release branch.** *How reviewers catch it:*
  `Cargo.lock` diff on the release PR is non-empty for reasons unrelated
  to the feature set.
- **Tried to make a signed `.pkg` / `.msi` reproducible.** *How reviewers
  catch it:* it can't be. The unsigned binary inside is; the signature
  envelope intentionally isn't.

## 8. FAQ

**Q: Does `--locked` affect codegen?**
A: No. It blocks dependency resolution changes. Codegen determinism is
`codegen-units = 1` plus the path/timestamp pins.

**Q: Why strip only debuginfo and not all symbols?**
A: Incident responders want the symbol table for core-file resolution;
dropping only DWARF keeps the symtab but drops the bulky debug info. See
`Cargo.toml:91`–`128` for the full rationale block.

**Q: Why `--build-id=none` and not the default SHA-1?**
A: `--build-id=none` is the simplest "always byte-identical" choice for
releases that do not ship to Fedora/debuginfod infrastructure. Distros
that need a build-id set `=sha1` in their packaging pipeline; SHA-1 over
final content is deterministic.

**Q: Does `cargo vendor` affect the build hash?**
A: It should not — `vendor/` content hashes are pinned by the lockfile.
If the vendored build hash differs from the registry-sourced one, that is
a reproducibility bug; capture it with `diffoscope` and file a bead.

**Q: Can I verify the macOS `.pkg` hash?**
A: You can verify the unsigned binary inside it. The outer signed `.pkg`
contains a timestamp-server response which is unique per signing run, so
the outer hash differs deliberately.

**Q: What about the AppImage?**
A: The squashfs payload is reproducible; the outer AppImage runtime stub
(shipped from upstream AppImageKit) is not our binary and is not covered.

**Q: We're pre-alpha — why invest in this now?**
A: Because retrofitting reproducibility after the first tag is painful.
Every release cut under a non-reproducible recipe has to be re-cut once
the contract is fixed, and that invalidates packager trust.
