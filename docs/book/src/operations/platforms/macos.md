# macOS

Platform notes for running `pcloud-daemon` and `pcloud-cli` on macOS.

## Support status

- **Scaffolded, not live-tested.** The macOS build compiles, the
  launchd plist ships, the `.pkg` signing pipeline is scripted, but
  no human has yet completed an end-to-end mount test on an actual
  macOS host. Treat mount behaviour as pre-alpha.
- See the canonical support matrix at
  [`architecture/platform-support.md`](../../architecture/platform-support.md).

> **Landing status (2026-04-15):** Tier 1 target, Tier 2 in practice
> until host bring-up. Phases P0–P5 are **wired, not yet live-verified**
> on a macOS host: the `fuse-t` adapter compiles with all 16 FUSE
> callbacks, the `.pkg` signing pipeline is scripted, and the launchd
> plist template ships. mounted-drive parity is still tracked under
> `bd-1du.4` until live verification lands. See
> [Packaging reference](../../reference/packaging.md) for Homebrew tap,
> MacPorts, and signed `.pkg` details including Apple notarisation.

## OS version matrix

| macOS            | Xcode CLT | Arch              | Status                             |
|------------------|-----------|-------------------|------------------------------------|
| 12 (Monterey)    | 14.x      | x86_64            | Build-only, no mount verification  |
| 13 (Ventura)     | 14.x–15.x | x86_64, arm64     | Build-only, no mount verification  |
| 14 (Sonoma)      | 15.x      | arm64 (primary)   | Build-only, no mount verification  |
| 15 (Sequoia)     | 16.x      | arm64             | Expected to build; untried         |
| <= 11 (Big Sur)  | any       | any               | **Not supported** — fuse-t gates on 12+ |

Apple Silicon is the primary target going forward. Intel builds
remain available via Homebrew and MacPorts but will fall out of
routine CI before end-of-fork.

## Install

### Package managers

```bash
# Homebrew (recommended)
brew tap pcloud-rs/pcloud-rs
brew install pcloud-rs

# MacPorts
sudo port install pcloud-rs
```

### Signed `.pkg`

Releases ship a notarized, signed `pcloud-rs-<version>.pkg` installer
for operators who distribute via MDM (Jamf, Kandji, Mosyle):

```bash
# Verify signature
pkgutil --check-signature pcloud-rs-<version>.pkg
spctl --assess --type install pcloud-rs-<version>.pkg

# Install (admin)
sudo installer -pkg pcloud-rs-<version>.pkg -target /
```

The `.pkg` installs binaries into `/usr/local/bin/` (Intel) or
`/opt/homebrew/bin/` (Apple Silicon) and drops a launchd plist template
into `/Library/LaunchAgents/` for fleet-managed per-user activation.

### From source

Build times on an M2 Pro (16 GiB RAM):

- Clean release build: **3–4 minutes.**
- Incremental recompile after touching one crate: **10–30 seconds.**

```bash
brew install rust fuse-t pkg-config
git clone https://github.com/pcloud-rs/pcloud-rs
cd pcloud-rs/
cargo build --release -p pcloud-daemon -p pcloud-cli
```

### Signing and notarization

Releases are signed with a Developer ID Application certificate and
notarized by Apple.

```bash
# Codesign (Developer ID Application)
bash packaging/signing/sign-macos.sh \
  --identity "Developer ID Application: <org> (<team id>)" \
  --entitlements packaging/macos/entitlements.plist \
  target/release/pcloud-daemon \
  target/release/pcloud-cli

# Notarize the built .pkg
bash packaging/signing/notarize-macos.sh \
  --bundle-id com.pcloud.pcloudd \
  --team-id <team id> \
  --apple-id <apple id> \
  pcloud-rs-<version>.pkg
```

The entitlements template at
`packaging/macos/entitlements.plist` intentionally keeps the
daemon sandbox-compatible — do not add `com.apple.security.files.all`
just to paper over a bug.

### Verification

```bash
shasum -a 256 -c SHA256SUMS.txt
codesign --verify --deep --strict --verbose=2 \
  $(command -v pcloud-daemon)
```

## Config paths (`~/Library/...`)

macOS uses Apple's container layout, not XDG, by default. The daemon
maps its roles as follows:

| Role               | Path                                                                              | Mode  |
|--------------------|-----------------------------------------------------------------------------------|-------|
| Config             | `~/Library/Application Support/com.pcloud.pcloudd/config.toml`                    | 0600  |
| State (store)      | `~/Library/Application Support/com.pcloud.pcloudd/store.sqlite`                   | 0600  |
| Vault              | `~/Library/Application Support/com.pcloud.pcloudd/vault.dat`                      | 0600  |
| Journal            | `~/Library/Application Support/com.pcloud.pcloudd/journal/`                       | 0700  |
| Cache              | `~/Library/Caches/com.pcloud.pcloudd/`                                            | 0700  |
| IPC socket         | `$TMPDIR/com.pcloud.pcloudd/daemon.sock` (per-user temp dir)                      | 0600  |
| Log (if file)      | `~/Library/Logs/com.pcloud.pcloudd/daemon.log`                                    | 0600  |

If `XDG_*` env vars are set, the daemon honors them too — useful when
you migrate scripts from Linux. The XDG path takes precedence; do not
mix both.

Create directories once:

```bash
install -d -m 0700 \
  "$HOME/Library/Application Support/com.pcloud.pcloudd" \
  "$HOME/Library/Application Support/com.pcloud.pcloudd/journal" \
  "$HOME/Library/Caches/com.pcloud.pcloudd" \
  "$HOME/Library/Logs/com.pcloud.pcloudd"
```

## Service management (launchd)

The daemon runs as a per-user LaunchAgent, not a LaunchDaemon — it
holds user secrets and per-user FUSE mounts.

Load the unit:

```bash
launchctl bootstrap gui/$(id -u) \
  ~/Library/LaunchAgents/com.pcloud.pcloudd.plist
launchctl enable gui/$(id -u)/com.pcloud.pcloudd
launchctl kickstart -k gui/$(id -u)/com.pcloud.pcloudd
```

Inspect:

```bash
launchctl print gui/$(id -u)/com.pcloud.pcloudd
launchctl print-disabled gui/$(id -u) | grep pcloudd
tail -f ~/Library/Logs/com.pcloud.pcloudd/daemon.log
```

Unload:

```bash
launchctl bootout gui/$(id -u)/com.pcloud.pcloudd
```

Minimal plist skeleton (shipped by the installer):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.pcloud.pcloudd</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/pcloud-daemon</string>
    <string>--log-format</string><string>json</string>
    <string>--log-level</string><string>info</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key>
  <dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Background</string>
  <key>StandardErrorPath</key>
  <string>/Users/__USER__/Library/Logs/com.pcloud.pcloudd/daemon.err</string>
  <key>StandardOutPath</key>
  <string>/Users/__USER__/Library/Logs/com.pcloud.pcloudd/daemon.out</string>
</dict>
</plist>
```

## Mount setup (fuse-t)

macOS no longer ships macFUSE by default (kernel-extension restrictions
since macOS 11). The supported backend is [**fuse-t**](https://www.fuse-t.org),
which bridges FUSE to NFS loopback and avoids kernel extensions entirely.

```bash
brew install --cask fuse-t
```

Verify:

```bash
pkgutil --pkg-info=io.fuse-t.pkg.core
mount | head -1   # sanity check the mount subsystem
```

macFUSE is accepted as a fallback for hosts that already depend on it,
but new deployments should choose fuse-t. Mixing macFUSE and fuse-t on
the same host is not supported.

Configure the mount:

```toml
[mount]
enabled = true
path    = "/Users/alice/pCloudDrive"
policy  = "default"
```

Wedged mount recovery — see
[runbook.md Playbook 7](../runbook.md#playbook-7-kernel-mount-recovery):

```bash
diskutil unmount force ~/pCloudDrive
# or:
sudo umount -f ~/pCloudDrive
# then:
launchctl kickstart -k gui/$(id -u)/com.pcloud.pcloudd
```

## Vault backend

On macOS the vault is a file under
`~/Library/Application Support/com.pcloud.pcloudd/vault.dat`,
mode `0600`, parent `0700`, UID-bound. There is **no** Keychain
integration at this time — adding a Keychain-backed vault is a
tracked future improvement. In-memory secrets are held in
`SecretString` / `SecretBytes` and zeroized on drop.

If you need hardware-backed root protection, FileVault provides the
equivalent of LUKS for the vault path; Apple Silicon's Secure Enclave
is not yet wired into the vault backend.

## Upgrade

See [Upgrade](../upgrade.md). Quick path with Homebrew:

```bash
pcloudc --json status > /tmp/pre.json
launchctl bootout gui/$(id -u)/com.pcloud.pcloudd
brew upgrade pcloud-rs
launchctl bootstrap gui/$(id -u) \
  ~/Library/LaunchAgents/com.pcloud.pcloudd.plist
pcloudc doctor --json
pcloudc status              # auth=Authenticated, healthy engine summary
```

## Uninstall

```bash
# 1. Stop and disable the launchd unit
launchctl bootout gui/$(id -u)/com.pcloud.pcloudd 2>/dev/null || true
rm -f ~/Library/LaunchAgents/com.pcloud.pcloudd.plist

# 2. Remove the package
brew uninstall pcloud-rs
# or, for .pkg-based installs:
sudo /usr/local/bin/pcloudc-uninstall.sh

# 3. Remove per-user state (this deletes the vault)
rm -rf \
  "$HOME/Library/Application Support/com.pcloud.pcloudd" \
  "$HOME/Library/Caches/com.pcloud.pcloudd" \
  "$HOME/Library/Logs/com.pcloud.pcloudd"

# 4. Remove runtime artifacts
rm -rf "$TMPDIR/com.pcloud.pcloudd"
```

Verify clean uninstall:

```bash
pgrep -a pcloud-daemon || echo "clean"
mount | grep -i pcloud || echo "clean"
launchctl print-disabled gui/$(id -u) | grep pcloudd || echo "clean"
```

## First-run bootstrap

Beginner path:

```bash
# 1. Install fuse-t via the no-kext cask; reboot NOT required.
brew install --cask fuse-t

# 2. Create the Application Support / Caches / Logs dirs once.
install -d -m 0700 \
  "$HOME/Library/Application Support/com.pcloud.pcloudd" \
  "$HOME/Library/Application Support/com.pcloud.pcloudd/journal" \
  "$HOME/Library/Caches/com.pcloud.pcloudd" \
  "$HOME/Library/Logs/com.pcloud.pcloudd"

# 3. Load the LaunchAgent.
launchctl bootstrap gui/$(id -u) \
  ~/Library/LaunchAgents/com.pcloud.pcloudd.plist
launchctl enable gui/$(id -u)/com.pcloud.pcloudd
launchctl kickstart -k gui/$(id -u)/com.pcloud.pcloudd

# 4. Sanity check
pcloudc doctor --json
pcloudc status
```

FAANG-ops tuning callouts:

- Use a LaunchAgent (not a LaunchDaemon) so the process inherits the
  user's login keychain context; daemons cannot access the per-user
  Keychain even if we later wire one up.
- Push the plist via MDM (Jamf, Kandji, Mosyle) and use an MDM
  configuration profile to pre-approve the fuse-t system extension so
  users never see the TCC prompt.
- Include the daemon binary in Endpoint Security allowlists; Santa
  rules should pin the Team ID on the Authenticode-equivalent.

## Service management cheat-sheet

| Action          | Command                                                             |
|-----------------|---------------------------------------------------------------------|
| Load            | `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.pcloud.pcloudd.plist` |
| Enable          | `launchctl enable gui/$(id -u)/com.pcloud.pcloudd`                  |
| Start/restart   | `launchctl kickstart -k gui/$(id -u)/com.pcloud.pcloudd`            |
| Stop            | `launchctl bootout gui/$(id -u)/com.pcloud.pcloudd`                 |
| Status          | `launchctl print gui/$(id -u)/com.pcloud.pcloudd`                   |
| Tail stdout     | `tail -f ~/Library/Logs/com.pcloud.pcloudd/daemon.out`              |
| Tail stderr     | `tail -f ~/Library/Logs/com.pcloud.pcloudd/daemon.err`              |
| Unified log     | `log stream --predicate 'process == "pcloud-daemon"' --info`         |
| Core-dump dir   | `/cores/` (`sudo launchctl limit core unlimited` as needed)         |

Core-dump capture:

```bash
sudo launchctl limit core unlimited
sudo chmod 1777 /cores
ulimit -c unlimited
```

## Peer-cred and IPC

- Transport: `AF_UNIX` stream socket under
  `$TMPDIR/com.pcloud.pcloudd/daemon.sock`, mode `0600`, parent dir
  `0700`, owner-checked on every bind.
- Peer identity: the daemon uses `getpeereid(3)` on macOS (and every
  other BSD-derived platform) to read the connecting peer's UID and
  GID. Non-matching UIDs are rejected with `peer.denied.uid_mismatch`.
- Unlike Linux, macOS `SO_PEERCRED` is not available; `getpeereid` is
  the supported path.
- `$TMPDIR` under macOS resolves to a per-user Darwin sandbox temp
  dir, so even other local users cannot reach the socket without
  explicit file-system access.

## Secret storage backend

- In-memory: `SecretString` / `SecretBytes` (zeroize-on-drop).
- On-disk: file-backed vault at
  `~/Library/Application Support/com.pcloud.pcloudd/vault.dat`,
  mode `0600`, parent `0700`, UID-bound.
- **Keychain is _not_ wired.** A Keychain-backed vault is a tracked
  follow-up. Until then, use FileVault to get hardware-assisted
  at-rest protection for the vault file.
- Apple Silicon's Secure Enclave is not yet used by the daemon. Do
  not assert Secure Enclave protection in release notes.

## Observability integration

- **Unified logging.** Everything written to stderr is captured by
  `launchd` and routed to `com.apple.Diagnostics` / Console.app.
- Structured JSON logs are written to
  `~/Library/Logs/com.pcloud.pcloudd/daemon.out`.
- `log stream --predicate 'process == "pcloud-daemon"' --info` gives
  live visibility, `log show --predicate '...' --last 1h` for
  retroactive queries.
- Crash reports: `~/Library/Logs/DiagnosticReports/` and
  `/Library/Logs/DiagnosticReports/` (admin).

## Gatekeeper, TCC, XProtect, and Defender-style EDR

- **Gatekeeper / notarization.** Any unsigned or unnotarized binary
  will be quarantined. The release pipeline signs + notarizes; local
  builds can be deblessed with `xattr -dr com.apple.quarantine`.
- **TCC.** The daemon does **not** need Full Disk Access for its own
  state; it only needs TCC grants for any sync-root outside
  `~/Library/Mobile Documents`. Adding the daemon to
  `System Settings → Privacy & Security → Files and Folders` is the
  right grant, not Full Disk Access.
- **fuse-t system extension.** macOS prompts the user the first time
  a process attaches to fuse-t. MDM pre-approval via `systemextensionsctl`
  is the fleet path.
- **XProtect / EDR.** Third-party tools (Jamf Protect, CrowdStrike,
  SentinelOne) tend to flag FUSE-like activity. Create an allowlist
  entry for `pcloud-daemon` by Team ID + bundle ID.

## Troubleshooting (top 10)

1. **`Operation not permitted` on mount** — TCC missing for the
   sync-root directory. Grant in System Settings.
2. **`The application cannot be opened`** — quarantine xattr from a
   non-notarised binary.
   ```bash
   xattr -dr com.apple.quarantine /usr/local/bin/pcloud-daemon
   ```
3. **`launchctl bootstrap` says `Load failed: 5: Input/output error`**
   — plist malformed. `plutil -lint ~/Library/LaunchAgents/com.pcloud.pcloudd.plist`.
4. **fuse-t prompt reappears every start** — system extension not
   approved. `systemextensionsctl list | grep -i fuse-t`.
5. **`EACCES` writing to `~/Library/Application Support/com.pcloud.pcloudd`**
   — wrong owner after Time Machine restore:
   ```bash
   sudo chown -R $(id -u):$(id -g) ~/Library/Application\ Support/com.pcloud.pcloudd
   chmod -R go-rwx ~/Library/Application\ Support/com.pcloud.pcloudd
   ```
6. **macFUSE conflict** — fuse-t refuses to co-exist. Uninstall
   macFUSE: `sudo /usr/local/bin/macfuse-uninstall`.
7. **`pcloudc status` says socket missing** — `$TMPDIR` scoping.
   Ensure the CLI is running under the same user session; `sudo
   pcloudc` will not see the user's socket.
8. **Clock drift → auth 401** — enable Apple NTP.
   ```bash
   sudo systemsetup -setusingnetworktime on
   ```
9. **Rosetta running arm64 binary** — reinstall the native arch:
   ```bash
   arch -arm64 brew reinstall pcloud-rs
   ```
10. **APFS snapshot sync root rejected** — expected; snapshots are
    read-only and not a valid sync target.

## Upgrading

- Homebrew: `brew upgrade pcloud-rs`. Restart the LaunchAgent.
- MDM `.pkg`: `installer -pkg pcloud-rs-<new>.pkg -target /`. The
  postinstall script kickstarts the LaunchAgent.
- Snapshot `~/Library/Application Support/com.pcloud.pcloudd/` before
  major-version bumps.

## Uninstalling

See the **Uninstall** section below for the step-by-step removal.

## Known gaps (macOS)

- No live mount verification yet — all fuse-t paths are wired but
  unexercised on real hardware.
- No Keychain integration.
- No native `LoginItem` integration; we ship a LaunchAgent instead.
- No Universal 2 binary yet — ship separate Intel and arm64 tarballs.
- Secure Enclave-backed keys are not used.

## Known issues

- **Gatekeeper / notarization.** Running an unsigned daemon on macOS
  12+ requires an explicit override
  (`System Settings → Privacy & Security → Open Anyway`). Always ship
  signed and notarized builds to end users.
- **Full Disk Access.** The daemon does **not** need FDA by default —
  the only filesystem it touches is the user's sync roots and its own
  Library paths. If users report permission errors reading their own
  files under `~/Documents`, add the daemon to FDA in
  System Settings → Privacy & Security → Full Disk Access.
- **fuse-t kext vs no-kext.** fuse-t is the no-kext path and is
  strongly preferred. If you see kernel-extension prompts, macFUSE is
  installed alongside — uninstall macFUSE to avoid conflicts.
- **Rosetta.** Apple Silicon users should install the native arm64
  binary. Running under Rosetta introduces measurable upload/download
  overhead and is not tested.
- **File events on APFS snapshots.** Sync-root registration onto a
  read-only APFS snapshot is rejected; snapshots are not a supported
  sync target.
- **Keychain not wired.** The vault is file-backed, not Keychain-
  backed. If your threat model requires a hardware-backed root, use
  FileVault and track the Keychain-integration bead.
