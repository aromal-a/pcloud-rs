# macOS packaging

This directory contains everything needed to build and distribute pcloud-rs on
macOS: installer scripts, disk-image scripts, code-signing/notarisation helpers,
and launchd property-list templates.

---

## Quick Start (macOS)

For a guided first-run experience:

```bash
./packaging/macos/first-run.sh
```

This script checks for fuse-t, installs binaries, sets up launchd, and logs you in.

## Scripts

| Script | Purpose |
|--------|---------|
| `install.sh` | Install binaries + LaunchAgent. Run `--build` to compile first. |
| `uninstall.sh` | Remove binaries and LaunchAgent. |
| `first-run.sh` | Interactive guided setup for new installations. |
| `setup-keychain.sh` | Check or clear pCloud credentials in the macOS Keychain. |
| `launchd-status.sh` | Show daemon status, plist validity, and recent logs. |
| `build-pkg.sh` | Build a macOS .pkg installer package. |
| `build-dmg.sh` | Build a macOS .dmg disk image. |

---

## Packaging options

### .pkg installer (`build-pkg.sh`)

Produces a standard macOS Installer package (`target/pkg/pcloud-rs-<version>-macos.pkg`).

The package installs:

- `/usr/local/bin/pcloudc` — command-line client
- `/usr/local/bin/pcloudd` — sync daemon
- `~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist` — user LaunchAgent

```sh
# Unsigned (development / CI):
./packaging/macos/build-pkg.sh

# Signed:
./packaging/macos/build-pkg.sh \
    --sign "Developer ID Installer: Acme Corp (ABCDE12345)"

# Signed + notarised (release):
APPLE_ID=you@example.com \
APPLE_APP_SPECIFIC_PASSWORD=xxxx-xxxx-xxxx-xxxx \
APPLE_TEAM_ID=ABCDE12345 \
./packaging/macos/build-pkg.sh \
    --sign "Developer ID Installer: Acme Corp (ABCDE12345)" \
    --notarize
```

Prerequisites: Xcode command-line tools (`pkgbuild`, `productbuild`,
`productsign`), Rust stable toolchain.

### .dmg disk image (`build-dmg.sh`)

Produces a compressed disk image (`target/pkg/pcloud-rs-<version>-macos.dmg`)
containing the two binaries and a `README-macOS.md` drawn from `docs/MACOS.md`.

```sh
# Unsigned:
./packaging/macos/build-dmg.sh

# Signed:
./packaging/macos/build-dmg.sh \
    --sign "Developer ID Application: Acme Corp (ABCDE12345)"
```

If `create-dmg` (installable via `brew install create-dmg`) is on `PATH`, the
script uses it for a window-sized, icon-positioned DMG. Otherwise it falls back
to the built-in `hdiutil`.

Prerequisites: `hdiutil` (built-in), optionally `create-dmg`, Rust stable
toolchain.

### Homebrew formula (`../homebrew/pcloud-rs.rb`)

See `packaging/homebrew/README.md` for the release process. The formula builds
from source and wires `pcloudd` as a `brew services`-managed LaunchAgent.

### Code signing and notarisation

`packaging/signing/sign-macos.sh` — signs a binary or bundle with a Developer
ID Application identity and enables the hardened runtime.

`packaging/signing/notarize-macos.sh` — submits a signed `.pkg`, `.dmg`, or
`.zip` to Apple's notary service, waits for approval, and staples the ticket.

Both scripts require:

| Variable | Purpose |
|----------|---------|
| `APPLE_ID` | Apple ID email address |
| `APPLE_APP_SPECIFIC_PASSWORD` | App-specific password (not the account password) |
| `APPLE_TEAM_ID` | 10-character Developer Team ID |

`packaging/macos/entitlements.plist` — entitlement set embedded during signing.
Review before adding capabilities; keep the set minimal for CLI tools.

---

## macOS launchd integration

This directory contains launchd property-list templates for running
pcloud-rs on macOS.

| File | Scope | Target directory |
|------|-------|------------------|
| `com.pcloud.pcloud-rs.plist` | User LaunchAgent (per-user, runs on login) | `~/Library/LaunchAgents/` |
| `com.pcloud.pcloudd.plist`  | System LaunchDaemon (runs at boot as `_pcloudd`) | `/Library/LaunchDaemons/` |

Both plists set `RunAtLoad`, a conservative `KeepAlive` (restart only
on crash / non-zero exit), separate `StandardOutPath` /
`StandardErrorPath` log files, and a `PCLOUD_*` `EnvironmentVariables`
block covering `PCLOUD_HOME`, `PCLOUD_CONFIG`, `PCLOUD_AUTH_VAULT`,
`PCLOUD_LOG_LEVEL`, `PCLOUD_MOUNT_POINT` (agent only),
`PCLOUD_IPC_SOCKET` (daemon only), and `PCLOUD_API_SERVER`.

Before installing the user agent, replace every `{{USER_HOME}}`
placeholder with an absolute path: launchd does not expand `$HOME`
inside plist values.

## User LaunchAgent

```sh
# 1. Substitute $HOME into the template.
sed "s|{{USER_HOME}}|$HOME|g" \
    packaging/macos/com.pcloud.pcloud-rs.plist \
    > ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist

# 2. Ensure the log directory exists.
mkdir -p ~/Library/Logs/pcloud-rs

# 3. Register and start.
launchctl load -w ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist

# Inspect:
launchctl list | grep com.pcloud.pcloud-rs
tail -f ~/Library/Logs/pcloud-rs/pcloud-rs.err.log

# Stop / uninstall:
launchctl unload -w ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist
rm ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist
```

## System LaunchDaemon

The daemon runs as the dedicated service account `_pcloudd`. Create it
once before installing the plist (see the install header comment in
`com.pcloud.pcloudd.plist`).

```sh
# 1. Install the plist with correct ownership and permissions.
sudo install -m 0644 -o root -g wheel \
    packaging/macos/com.pcloud.pcloudd.plist \
    /Library/LaunchDaemons/com.pcloud.pcloudd.plist

# 2. Prepare state and log directories.
sudo install -d -o _pcloudd -g _pcloudd -m 0700 /var/lib/pcloudd
sudo install -d -o _pcloudd -g _pcloudd -m 0750 /var/log/pcloudd
sudo install -d -o _pcloudd -g _pcloudd -m 0755 /var/run/pcloudd

# 3. Register and start.
sudo launchctl load -w /Library/LaunchDaemons/com.pcloud.pcloudd.plist

# Inspect:
sudo launchctl list | grep com.pcloud.pcloudd
sudo tail -f /var/log/pcloudd/pcloudd.err.log

# Stop / uninstall:
sudo launchctl unload -w /Library/LaunchDaemons/com.pcloud.pcloudd.plist
sudo rm /Library/LaunchDaemons/com.pcloud.pcloudd.plist
```

## Notes

- The agent plist keeps `ProcessType` set to `Interactive` because it
  may mount a FUSE volume visible in Finder; the daemon is `Background`.
- Do not set `PCLOUD_INSECURE_HTTP` or any other transport-weakening
  variable in these files; the production Rust build rejects plaintext.
- Auth-vault permissions are still enforced by the daemon itself
  (`0600` file, `0700` parent) regardless of what is configured here.
