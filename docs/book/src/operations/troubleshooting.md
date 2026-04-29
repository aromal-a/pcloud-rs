# Troubleshooting

Cross-cutting failure modes for `pcloud-daemon`, `pcloud-cli`, and the
mounted-drive runtime. This chapter collects the symptoms most likely
to be seen in the field, paired with diagnosis steps and resolutions.
For platform-specific top-10 lists, see the corresponding chapters in
[`operations/platforms/`](./platforms/linux.md).

> **Honesty callout.** The Rust rewrite still has open parity gaps
> (see [`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md)
> and the [Parity Status](../parity/status.md) chapter). FUSE writes
> are live-verified on Linux only; macOS and Windows mounts are
> scaffolded but not yet hardware-verified. Several remediation steps
> below assume you have inspected daemon logs first — none of these
> recipes should be applied blindly to a working daemon.

---

## 1. FUSE / mounted-drive failures

### 1.1 `Transport endpoint is not connected`

**Symptom.** A path under the mountpoint returns `ENOTCONN` /
"Transport endpoint is not connected" on `ls`, `stat`, or `read`.

**Cause.** The kernel still has an FUSE mount registered, but the
user-space daemon that was servicing it has exited (crash, SIGKILL,
container OOM, or an aborted `unmount`). The mountpoint is now
"orphaned": the kernel keeps the dentry cached but every dispatch
fails because the FUSE channel has been closed.

**Diagnosis.**

```bash
# Confirm the orphan mount is still registered with the kernel.
findmnt -t fuse,fuse.fuse-t
mount | grep fuse

# Confirm there is no live daemon process attached to it.
pgrep -af pcloud-daemon
fuser -v /path/to/mountpoint   # should print nothing
```

**Resolution.**

```bash
# Lazy-detach is the safe default; the kernel reaps the entry once the
# last user-space file descriptor closes.
fusermount3 -u /path/to/mountpoint        # Linux libfuse3
fusermount  -u /path/to/mountpoint        # libfuse2 fallback
sudo umount -l /path/to/mountpoint        # last resort

# macOS:
diskutil unmount force /path/to/mountpoint

# Windows (WinFSP):
sc stop pcloudd-svc
# Then restart the service.
```

After unmount, restart the daemon and re-mount. The next mount creates
a fresh FUSE channel.

> **Background.** Linux now reaps stale mounts on `SIGTERM`/`SIGINT`
> via the daemon's signal handler (`crates/pcloud-fs/src/platform/
> linux.rs::reap_all_mounts`). On BSD and Windows the reaper is Tier-3
> — operators should expect to perform manual cleanup if the daemon
> dies abruptly. See `CLAUDE.md` § "Signal-driven mount cleanup
> posture" for the full posture.

### 1.2 `fusermount3: not found` / no FUSE binary on PATH

**Symptom.** Mount fails immediately with `Error: fusermount3: not
found in PATH` or similar.

**Cause.** libfuse3 user-space helpers are not installed.

**Resolution.** Install the distro package:

```bash
# Debian/Ubuntu
sudo apt-get install fuse3

# Fedora/RHEL
sudo dnf install fuse3

# Arch
sudo pacman -S fuse3

# Alpine
apk add fuse3
```

Confirm `fusermount3 --version` resolves. The daemon prefers
`fusermount3` over `fusermount`; if only the legacy binary is
present, mount may still succeed but error messages will reference
the v2 path.

### 1.3 `allow_other` rejected by the kernel

**Symptom.** Mount fails with `option allow_other only allowed if
'user_allow_other' is set in /etc/fuse.conf` or similar.

**Cause.** The daemon was instructed to expose the mount to other
local UIDs (`mount.allow_other = true`), but the kernel's FUSE
helper enforces an opt-in flag.

**Resolution.**

1. Decide whether `allow_other` is required. The Rust default is
   **denied** (single-UID visibility); only enable it if the mount
   must be visible to a service user other than the daemon UID.
2. If yes, add to `/etc/fuse.conf`:

   ```
   user_allow_other
   ```

3. Re-mount. If you do not control `/etc/fuse.conf` (managed image,
   container), set `mount.allow_other = false` in `config.json` and
   bind-mount instead from the daemon-owned UID.

> **Security note.** `allow_other` widens the daemon's threat
> surface. The default-deny posture is intentional. See
> [`security/threat-model.md`](../security/threat-model.md).

### 1.4 Stale mount after a daemon crash

**Symptom.** `mount` lists `/path/to/mountpoint` as mounted, but
`pcloud-daemon` is not running. New `pcloudd serve` start aborts
with `mountpoint already in use`.

**Cause.** Daemon process died before its mount-cleanup signal
handler ran (e.g. SIGKILL, OOM, kernel panic).

**Resolution.**

```bash
# 1. Confirm the orphan
findmnt /path/to/mountpoint

# 2. Detach
fusermount3 -u /path/to/mountpoint
# or
sudo umount -l /path/to/mountpoint

# 3. Inspect daemon logs (systemd or journal)
journalctl --user -u pcloudd.service -n 200
# system mode:
sudo journalctl -u pcloudd.service -n 200

# 4. Restart cleanly
systemctl --user restart pcloudd.service
```

If this happens repeatedly, capture a coredump via
`coredumpctl gdb pcloud-daemon` and file an issue with the trace.

### 1.5 Permission denied opening `/dev/fuse`

**Symptom.** `Permission denied (os error 13)` when opening
`/dev/fuse` during mount setup.

**Cause.** The shipped systemd unit blocks `/dev/fuse` and the
`@mount` syscall group by default (`PrivateDevices=yes`,
`SystemCallFilter=~@mount`). FUSE deployments **must** install the
ship-with `override-fuse.conf.example` drop-in.

**Resolution.**

```bash
sudo systemctl edit pcloudd.service
# paste contents of packaging/systemd/override-fuse.conf.example
sudo systemctl daemon-reload
sudo systemctl restart pcloudd.service
```

See [`packaging/systemd/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/README.md)
for the trade-off discussion (`PrivateDevices=yes` is dropped under
the override; the syscall filter is widened to allow `mount`).

---

## 2. Vault corruption / vault locked

### 2.1 Indicators

- `pcloud-cli login` repeatedly prompts even after a successful run.
- Daemon log emits `vault: refusing to open: mode=0644 expected=0600`
  or `vault: refusing to open: owner=root expected=<your-uid>`.
- `pcloud-cli userinfo` returns `Unauthenticated` immediately after
  login.
- The vault file is short, zero-byte, or the daemon reports a
  deserialization error.

### 2.2 Recovery

The vault stores **opt-in** durable auth tokens; passwords are never
persisted. Wiping the vault forces a fresh login but loses no other
state — the SQLite store, journal, sync roots, and crypto material
all survive.

```bash
# 1. Stop the daemon.
systemctl --user stop pcloudd.service

# 2. Locate the vault. (Linux defaults; see platforms/linux.md.)
ls -la "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud-rs/auth-vault.json"

# 3. Verify ownership and mode.
stat -c '%U %a %n' "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud-rs/auth-vault.json"
# Expected: <your-username> 600 .../auth-vault.json

# 4. Repair if drift is the only issue.
chmod 600 "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud-rs/auth-vault.json"
chmod 700 "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud-rs"

# 5. If the vault is corrupt, remove it and re-login.
rm "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud-rs/auth-vault.json"
systemctl --user start pcloudd.service
pcloud-cli login
```

What survives a vault wipe:

- the SQLite store (sync-root list, queued work, journal)
- the audit log (append-only; integrity hash chain preserved)
- the crypto profile (AES key wrap, KDF parameters, fingerprint)

What is lost:

- any cached refresh tokens; you must complete the auth + TFA flow
  from scratch.

> **Hard rule.** The daemon **does not** silently repair vault
> ownership or mode. If `stat` shows drift, the daemon refuses to
> open the vault until you repair it manually. This is intentional —
> see ADR 0015.

---

## 3. Sync queue stuck

### 3.1 Symptoms

- A pending upload or download has been queued for more than an hour.
- `pcloud-cli sync status` shows `queued = N` but `in-flight = 0`.
- Disk use under the staging directory is growing without bound.

### 3.2 Inspection

```bash
# Daemon-level queue inspection (CLI must be authenticated).
pcloud-cli sync list
pcloud-cli sync status --root <name>

# Inspect the on-disk SQLite store directly (read-only).
sqlite3 -readonly \
  "${XDG_STATE_HOME:-$HOME/.local/state}/pcloud-rs/store.sqlite" \
  '.tables'

# Look at what is in the upload queue.
sqlite3 -readonly \
  "${XDG_STATE_HOME:-$HOME/.local/state}/pcloud-rs/store.sqlite" \
  'SELECT id, kind, state, attempts, last_error FROM queue
   WHERE state != "complete" ORDER BY id DESC LIMIT 50;'

# Inspect the journal (writeback intents).
ls -la "${XDG_STATE_HOME:-$HOME/.local/state}/pcloud-rs/journal/"
```

### 3.3 Resolution

Step 1: identify the stuck item by `last_error`. Common categories:

- `auth.unauthenticated` — vault expired; re-run `pcloud-cli login`.
- `transfer.checksum_mismatch` — local file changed mid-upload; the
  daemon will retry once it re-reads the source. If the source no
  longer exists, manually drop the queue entry (see below).
- `transfer.network` — transient. The daemon retries with backoff.
- `mount.staging_full` — the staging directory hit its quota; free
  space and the next sweep tick picks the work up.

Step 2: force a sync sweep.

```bash
pcloud-cli sync sweep --root <name>
```

Step 3: replay the journal if a writeback intent is wedged.

```bash
pcloud-cli sync journal-replay --root <name>
```

Step 4: as a last resort, drop a single stuck item. **This loses the
queued work** — the source file must be re-touched or re-copied for
the daemon to re-detect it.

```bash
# Get its id from the SELECT above, then:
pcloud-cli sync drop --queue-id <id>
```

If the queue is stuck **across** all roots, the daemon engine itself
may be hung. Capture a backtrace before restarting:

```bash
sudo gdb -p $(pgrep -f pcloud-daemon) \
  -ex 'set pagination off' -ex 'thread apply all bt' -ex quit
systemctl --user restart pcloudd.service
```

---

## 4. TLS pinning mismatch / certificate errors

### 4.1 Symptoms

- Daemon log shows `tls: server certificate signature verification
  failed` or `tls: peer offered an unsupported certificate type`.
- All API calls fail with `transport.tls`.
- `pcloud-cli userinfo` succeeds against `eapi.pcloud.com` but fails
  against `api.pcloud.com` (or vice versa).

### 4.2 Diagnosis

```bash
# Confirm the daemon's view of the API endpoint.
pcloud-cli config get api-server

# Independently fetch the live certificate fingerprint.
echo | openssl s_client -servername api.pcloud.com \
  -connect api.pcloud.com:443 2>/dev/null \
  | openssl x509 -noout -fingerprint -sha256

# Cross-check against the system trust store.
curl -v https://api.pcloud.com/userinfo 2>&1 | grep -E 'subject|issuer|verify'
```

### 4.3 Causes and resolution

| Cause | Action |
|-------|--------|
| Endpoint switched (e.g. EU vs US) | `pcloud-cli config set api-server eapi.pcloud.com` (or `api.pcloud.com`); restart daemon. |
| Corporate MITM proxy injects its own CA | Add the corporate CA to the system trust store. The daemon uses `rustls-native-certs` and will pick it up. **Do not** disable validation; production builds reject every transport-validation bypass (ADR 0004). |
| Outdated system root bundle | `update-ca-certificates` (Debian/Ubuntu), `update-ca-trust` (RHEL/Fedora). |
| Suspected MITM | Compare the live fingerprint against a known-good fingerprint from a different network (mobile hotspot). If they differ, **stop the daemon** and treat the network as hostile. |

Production builds **never** fall back to plaintext, and there is no
"insecure" config flag. The only legitimate use of `--api-server` is
to switch between published pCloud regional endpoints.

---

## 5. Two-factor authentication failures

### 5.1 SMS / device-push code never arrives

```bash
# Re-trigger device push.
pcloud-cli login --resend-tfa-device

# Or SMS:
pcloud-cli login --resend-tfa-sms
```

If neither arrives within 60 seconds, fall back to a recovery code:

```bash
pcloud-cli login --recovery-code <CODE>
```

### 5.2 Common error codes

| Code | Meaning | Action |
|------|---------|--------|
| 2074 | TFA token expired | Restart `pcloud-cli login`; the token is single-use and ~5-minute-lived. |
| 2075 | TFA code invalid | Re-enter; do not reuse a previously valid code. |
| 2076 | Rate-limit / too many attempts | Wait 5 minutes; do not retry in a loop. |
| 2092 | Recovery code already used | Use a different recovery code; one-time-use only. |
| 2306 | Device not yet trusted | Submit the device-push or SMS code first; the device is registered post-login. |

The recovery-code path bypasses the device push entirely and is the
correct fallback when the user has lost access to the registered
device.

### 5.3 "Stuck" TFA loop

If `pcloud-cli login` re-prompts for TFA on every run:

1. Confirm the vault is being persisted: see [§ 2 above](#2-vault-corruption--vault-locked).
2. Confirm token persistence is enabled (it is **opt-in**):
   ```bash
   pcloud-cli config get auth.persist-token
   # If false, durable auth is intentionally disabled; every login
   # is a fresh TFA round.
   ```
3. If you want durable login, opt-in:
   ```bash
   pcloud-cli config set auth.persist-token true
   pcloud-cli login
   ```

---

## 6. Crypto unlock failure

### 6.1 Wrong-password vs backend mismatch

The daemon supports two crypto backends with **incompatible** wire
formats:

- `PclsyncCompat` — byte-compatible with the official pCloud apps.
- `Enhanced` — stricter AEAD (AES-256-GCM) + Argon2id. Opt-in via
  `--acknowledge-not-interop`. **Not** interoperable with pCloud.

The active backend is recorded in the crypto profile metadata. A
cross-backend unlock fails fast with `BackendMismatch` and **never**
falls back silently.

| Daemon log | Meaning | Action |
|------------|---------|--------|
| `crypto: unlock failed: WrongPassword` | Password is wrong, backend is correct. | Retry with the correct password. |
| `crypto: unlock failed: BackendMismatch (profile=PclsyncCompat, request=Enhanced)` | The profile was created with the pCloud-compatible backend; you tried to unlock with `Enhanced`. | Use `pcloud-cli crypto unlock` (default = `PclsyncCompat`). |
| `crypto: unlock failed: BackendMismatch (profile=Enhanced, request=PclsyncCompat)` | The profile was created with `Enhanced`; you tried to unlock with the default. | Re-run with `--backend enhanced --acknowledge-not-interop`. |
| `crypto: unlock failed: ProfileMissing` | Crypto has never been set up on this account from this client. | Run `pcloud-cli crypto setup`. |

### 6.2 Forgotten crypto password

There is **no recovery path**. The crypto password derives the master
key; pCloud cannot reset it without destroying every encrypted file
in the crypto folder. To regain access, you must:

1. Run `pcloud-cli crypto reset` (destroys all encrypted folders /
   files in the cloud).
2. Re-run `pcloud-cli crypto setup` with a new password.

Confirm the action; this is irreversible.

---

## 7. Permission errors on socket / vault / mount

### 7.1 What the modes mean

| Path | Mode | Purpose |
|------|------|---------|
| `<runtime>/pcloud-rs/` | `0700` | Runtime dir, owner-only. |
| `<runtime>/pcloud-rs/pcloudd.sock` | `0600` | Local IPC socket, owner-only; `SO_PEERCRED` enforced on accept. |
| `<state>/pcloud-rs/` | `0700` | State dir (SQLite, journal, vault). |
| `<state>/pcloud-rs/auth-vault.json` | `0600` | Auth vault. Daemon refuses to open if mode or owner drifts. |
| Mountpoint root | inherited from `mkdir` | The daemon does **not** chmod a user-supplied mountpoint. |

### 7.2 Repair

```bash
# Linux/macOS (BSD: same).
chmod 700 "${XDG_RUNTIME_DIR:-/run/user/$UID}/pcloud-rs"
chmod 600 "${XDG_RUNTIME_DIR:-/run/user/$UID}/pcloud-rs/pcloudd.sock"
chmod 700 "${XDG_STATE_HOME:-$HOME/.local/state}/pcloud-rs"
chmod 600 "${XDG_STATE_HOME:-$HOME/.local/state}/pcloud-rs/auth-vault.json"

# Confirm ownership (must match the daemon UID).
stat -c '%U %a %n' \
  "${XDG_STATE_HOME:-$HOME/.local/state}/pcloud-rs/auth-vault.json"
```

If `chown` is necessary (e.g. after copying state from another host),
chown the entire `pcloud-rs/` tree to the daemon UID — the daemon
walks every state file at startup and refuses to open any that drift.

### 7.3 Windows specifics

NTFS ACLs replace POSIX modes. The vault parent directory is
inheritance-locked to the user's SID at install time. If you see
`vault: ACL drift detected`, run:

```powershell
icacls "%LOCALAPPDATA%\pcloud-rs" /reset /t
icacls "%LOCALAPPDATA%\pcloud-rs" /grant:r "%USERNAME%:(OI)(CI)F" /inheritance:r
```

See [`platforms/windows.md`](./platforms/windows.md) for the SID-
restoration walkthrough.

---

## 8. Where to capture diagnostics before filing an issue

When opening a bug report:

1. Daemon version: `pcloudd --version`.
2. Last 500 lines of the daemon log:
   ```bash
   journalctl --user -u pcloudd.service -n 500 --no-pager > daemon.log
   ```
3. State-dir listing (no contents): `ls -laR <state-dir> > state.txt`.
4. SQLite schema version:
   ```bash
   sqlite3 -readonly <state-dir>/store.sqlite 'PRAGMA user_version;'
   ```
5. If a mount is involved: `findmnt -t fuse,fuse.fuse-t > mounts.txt`.
6. Crypto fingerprint (safe to share; not the password):
   `pcloud-cli crypto fingerprint`.

**Never** share the vault file, the password, or any HTTPS request
captures. The daemon's structured logs are pre-redacted; the bare
state directory is not.
