# Reproducible Builds

> **Status honesty:** **pre-alpha**. No release tag exists yet; the CI
> double-build job and the published checksum/signature files described below
> are the *contracts* we hold ourselves to for the first tag, not empirical
> artefacts downstream auditors can already verify. Treat this page as a
> binding specification that the release-cutting PR will exercise end-to-end.

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
- the exact verification procedure a third party runs to confirm the
  workflow-emitted checksum and signature files match a locally rebuilt
  auditable binary.

## 2. Prerequisites

### Toolchain pin

- `rust-toolchain.toml` — exactly pins `channel = "1.91.0"`, with
  `clippy, rustfmt`. Release, packaging, and cross-host reproducibility
  workflows use the same exact compiler:

  ```sh
  rustup show          # must select 1.91.0 for this checkout
  ```

- Portable workspace/core: `edition = "2024"`, `rust-version = "1.89"`.
  The isolated `pcloud-plugin-wasmtime` crate declares Rust 1.91 because its
  advisory-fixed Wasmtime 43 dependency requires that compiler; it is not part
  of the distributed daemon/CLI build.
- Build profile: `--profile release-repro` (defined in `Cargo.toml`,
  inherits from `release-dist`; `strip = "symbols"`, `debug = false`,
  `codegen-units = 1`). The core rationale block is at `Cargo.toml:91`.

### System dependencies

- `git` (to recover the tag commit time for `SOURCE_DATE_EPOCH`).
- `cosign` (to verify the detached signatures emitted by `release.yml`).
- `gpg` only if a future release also publishes `SHA256SUMS.asc`.
- `cargo-auditable` — required to match the release workflow's auditable
  binary format.
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
 cargo auditable build --profile release-repro --locked
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
# strip symbols and DWARF; symbolication uses the matching unstripped build.
strip = "symbols"
debug = false
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

`rust-toolchain.toml` — commit the exact release compiler. Rebuilders install:

```sh
rustup install 1.91.0
rustup component add --toolchain 1.91.0 clippy rustfmt
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
nix build --refresh .#pcloud-rs-repro --no-write-lock-file
```

The lock file fully pins every input including nixpkgs revision, so the
build graph is reproducible end-to-end. Current flake outputs use the
`release-repro` profile and deterministic flags, but they do not invoke
`cargo auditable`; do not compare them byte-for-byte with signed release
assets until that gap is closed.

## 5. Verification procedure

### 5.1 One-shot script (recommended)

The repo ships a wrapper at
`packaging/scripts/verify-reproducibility.sh` that performs steps 1–5 below
for `pcloudc` and `pcloudd` together with `cargo auditable`.
`cargo xtask release` invokes the same local reproducibility gate after all
CI stages pass.

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
- runs `cargo auditable build --locked --profile release-repro -p
  pcloud-cli -p pcloud-daemon` twice (first build,
  `cargo clean --profile release-repro`, second build),
- snapshots each build's `pcloudc` / `pcloudd` under a temp directory and
  compares SHA-256 manifests,
- exits 0 only if both binaries match byte-for-byte across the two runs.

### 5.2 Manual procedure

The local release gate runs the equivalent auditable build twice. A third
party may reproduce it by hand as follows:

```sh
git checkout v<version>

# 1. Pin build-time metadata.
export SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct)
export RUSTFLAGS='--remap-path-prefix=$PWD= -C link-arg=-Wl,--build-id=none'

# 2. Confirm toolchain pin.
rustc --version    # must match rust-toolchain.toml exactly
cargo auditable --version

# 3. Build.
CARGO_PROFILE_RELEASE_REPRO_ACTIVE=1 \
  cargo auditable build --profile release-repro --locked \
    -p pcloud-cli -p pcloud-daemon
sha256sum target/release-repro/pcloudc target/release-repro/pcloudd > /tmp/first.sha

# 4. Scrub and rebuild.
cargo clean --profile release-repro
CARGO_PROFILE_RELEASE_REPRO_ACTIVE=1 \
  cargo auditable build --profile release-repro --locked \
    -p pcloud-cli -p pcloud-daemon
sha256sum target/release-repro/pcloudc target/release-repro/pcloudd > /tmp/second.sha

# 5. Compare.
diff /tmp/first.sha /tmp/second.sha       # must be empty
```

Then compare against the operator-published signed checksum files using the
verification procedure for the selected signing provider, followed by
`sha256sum -c pcloudc.sha256 --ignore-missing`.

If the local hash matches the signed checksum, the release artefact is
verified end-to-end. If it does not, capture the divergence with
`diffoscope` and open a bead tagged `repro-regression`.

## 6. Gate checklist

On the release branch:

- [ ] `rust-toolchain.toml` pin unchanged since the previous release.
- [ ] `Cargo.lock` committed on the release tag.
- [ ] `Cargo.toml` carries `[profile.release-repro]` with
      `codegen-units = 1`, `strip = "symbols"`, `debug = false`.
- [ ] `SOURCE_DATE_EPOCH` exported from the tag commit time in the local
      release environment.
- [ ] `RUSTFLAGS` carries `--remap-path-prefix` for the runner root and
      `$CARGO_HOME` / `$HOME/.rustup`.
- [ ] `-C link-arg=-Wl,--build-id=none` (or `=sha1` per distro policy).
- [ ] Double-build job green: two independent
      `cargo auditable build --profile release-repro` runs produce
      identical SHA-256 manifests for `pcloudc` and `pcloudd`.
- [ ] Operator-produced signature files verify for raw binaries and SBOMs.
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
- **Used plain `cargo build` instead of `cargo auditable build`.** *How
  reviewers catch it:* the local manifest may be self-consistent but will
  not match the release workflow's auditable binary.
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

## 9. Cross-platform reproducibility (macOS + Windows)

The §1–§7 contract is written from the Linux ELF perspective because the
current local double-build driver is Linux-native. The former GitHub macOS
and Windows definitions are archived and inactive. Native cross-platform
reproducibility remains a release qualification step that must run on
operator-provided macOS and Windows hosts.

### 9.1 Files involved

- `.github/workflows-disabled/repro-build-macos.yml` — archived reference
  for the former two-slot macOS build.
- `.github/workflows-disabled/repro-build-windows.yml` — archived reference
  for the former Windows build and its `link.exe /Brepro` flag (see §9.3).
- `scripts/diff-repro-builds.sh` — Bash helper that hashes a fixed
  basename list across two directories and exits non-zero on
  divergence. Auto-detects `sha256sum` / `shasum -a 256` / `certutil`
  so operators can run it identically on Linux, macOS, and git-bash on
  Windows.
- `packaging/scripts/verify-reproducibility.sh` — Linux-only build
  driver from §5.1, invoked by `cargo xtask release`.

### 9.2 macOS specifics

macOS uses the `ld64` linker from Apple's toolchain. The GNU-ld
`--build-id=none` flag is a no-op on Mach-O — Mach-O objects do not carry
an ELF-style build-id note. Determinism of the Mach-O `LC_UUID` load
command is handled by `ld64` itself when `codegen-units = 1` and inputs
are deterministic; older `ld64` versions used a random UUID, but
toolchains shipped with macOS-latest runners on GitHub Actions hash the
content. We therefore drop `-Wl,--build-id=*` on macOS and rely on:

- `--profile release-repro` (`codegen-units = 1`, `strip = "symbols"`,
  `debug = false`),
- `SOURCE_DATE_EPOCH=1700000000` exported at the workflow level (a fixed
  constant — both matrix slots see the same value, so wall-clock-derived
  timestamps cannot leak),
- `RUSTFLAGS="--remap-path-prefix=${GITHUB_WORKSPACE}="` to scrub the
  runner checkout path out of debug strings,
- `cargo auditable build --locked` to match the auditable-binary format
  used by the release pipeline.

If the macOS job fails byte-equality, the slot-a and slot-b artefacts
are retained for 90 days; download both and run `diffoscope` locally.

### 9.3 Windows specifics — the PE timestamp trap

Windows PE binaries have **three** non-determinism sources that ELF does
not. They will silently break reproducibility unless mitigated:

1. **PE COFF `TimeDateStamp`.** `link.exe` writes the current wall-clock
   time into the file header on every link. `SOURCE_DATE_EPOCH` does
   *not* fix this — it is honoured by rustc for embedded crate metadata
   but not by MSVC link.exe.
2. **Debug directory `TimeDateStamp`.** Same wall-clock value is written
   into the PE debug directory entry that points at the PDB.
3. **`rc.exe` resource embedding.** When a resource has no explicit
   `FileVersion` stamp, the resource compiler embeds a build timestamp.
   Currently a non-issue for this tree (no `*.rc` files), but worth
   recording.

The MSVC mitigation is a single linker flag:

```text
/Brepro
```

Documented at *Microsoft Learn → /Brepro (Output Replicable Binaries)*:
when present, link.exe replaces both `TimeDateStamp` fields with the
sentinel value `0xFFFFFFFF` and zeroes related fields, so two builds
from identical inputs are byte-identical. We pass it through Rust via:

```text
RUSTFLAGS="-C link-arg=/Brepro --remap-path-prefix=${GITHUB_WORKSPACE}="
```

The `--remap-path-prefix` half is the same as on Linux/macOS — scrubs
absolute checkout paths out of debug strings.

What we deliberately do *not* do on Windows:

- We do not pass `--build-id=none`. That is a GNU-ld flag and link.exe
  rejects it; the Windows-equivalent guarantee is `/Brepro`.
- We do not produce a PDB in `release-repro` (the profile sets
  `debug = false`). PDB GUIDs are reproducibility hazards even with
  `/Brepro`; a future "release-repro-with-pdb" profile would need
  `/PDBALTPATH` plus deterministic-PDB tooling.
- We do not run `signtool`. Signed `.exe` / `.msi` outer envelopes are
  not reproducible by design (timestamp-server response is unique per
  signing call). The unsigned binary is.

### 9.4 Workflow-level flags summary

| Variable / flag | Linux | macOS | Windows |
| --- | --- | --- | --- |
| `SOURCE_DATE_EPOCH` | tag commit time (CI) / `1700000000` (cross-platform) | `1700000000` | `1700000000` |
| `--remap-path-prefix=${GITHUB_WORKSPACE}=` | yes | yes | yes |
| `-C link-arg=-Wl,--build-id=none` | yes | no (Mach-O) | no (rejected by link.exe) |
| `-C link-arg=/Brepro` | no | no | **yes** |
| `--profile release-repro` | yes | yes | yes |
| `cargo auditable build --locked` | yes | yes | yes |
| Two-runner-context diff via `diff-repro-builds.sh` | yes (`ci.yml`) | yes (`repro-build-macos.yml`) | yes (`repro-build-windows.yml`) |

### 9.5 Status of cross-platform reproducibility — T3.5

Per `CLAUDEREV/TIER-PROGRESS.md` row T3.5, this is **PARTIAL** until a
real macOS + Windows runner pair successfully completes a two-slot
build whose SHA-256 manifests match. The AI-scope foundation — the two
workflows, the helper script, the Windows `/Brepro` mitigation, the
artefact-retention contract — is landed. The empirical proof requires
infrastructure (user-provided runners) outside this campaign's scope.

When closing T3.5: re-run both workflows, attach the green run links to
the bead comment, and flip the row to DONE.

## 10. FAQ

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
