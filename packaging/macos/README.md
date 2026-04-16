# macOS launchd integration

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
