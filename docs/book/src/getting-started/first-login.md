# First Login

> **TL;DR** — start the daemon, log in, prove you're in:
>
> ```bash
> systemctl --user start pcloudd          # or `pcloudc start`
> pcloudc login -u you@example.com               # interactive; prompts for password + 2FA
> printf '%s' "$PASS" | pcloudc login \
>   --user you@example.com --password-stdin      # scripted alternative
> pcloudc status                                 # verify: "authenticated as you@example.com"
> ```
>
> Raw passwords are **never written to disk**, no matter which flag
> you pass. `--save-password` persists the *auth token*, not the
> password, and only when `PCLOUD_DURABLE_AUTH_TOKENS=1` is set.

## What you'll learn

- How to start and verify the daemon on Linux, macOS, Windows, and
  ad-hoc foreground mode.
- The exact IPC messages the CLI exchanges with the daemon during
  login (useful when tracing or debugging).
- Three scripted login patterns, in security-preference order:
  `--password-stdin`, `--password-env`, durable token vault.
- Every 2FA fallback: TOTP, SMS, push notification, recovery code —
  which one you should prefer for which account type.
- The stable exit-code contract — how to tell an invalid password
  from rate limiting, clock drift, or a broken network.
- How to log out cleanly, including secure purge of the token vault.

## Conceptual background

**Auth in pCloud is token-based.** You present a username + password;
the API returns a bearer **auth token** scoped to your account. Every
subsequent API call carries that token. There is no cookie, no OAuth
refresh dance — the token is long-lived, and the only reason to
re-authenticate is if it gets revoked or expires server-side.

Two things flow from that:

1. **The token is the secret.** Protect it like you would protect a
   long-lived SSH key. That is why the optional on-disk vault is
   owner-only (`0600`) inside an owner-only parent (`0700`), why the
   daemon validates ownership + mode on every read, and why the vault
   is opt-in behind an environment variable *and* a CLI flag.
2. **Passwords never need to touch disk.** The C client used to
   persist the raw password to make unattended restart possible. We
   deliberately **do not mirror that behaviour**. If you need
   unattended restart, persist the token; if you need the password,
   use a proper secret manager and feed it via `--password-stdin`.

**2FA** adds a second challenge to the password exchange. pCloud
supports four methods:

- **TOTP** — the 6-digit code from an authenticator app (Authy, 1Password,
  Google Authenticator, etc). This is the default and the fastest.
- **SMS** — a code texted to your registered phone. Slower and subject
  to SIM-swap attacks; use only when TOTP is unavailable.
- **Push** — a tap on an approved pCloud mobile app. Convenient and
  phishing-resistant.
- **Recovery code** — one of the one-time codes you printed when you
  set up 2FA. Use when you have lost every other factor.

For **business / team** accounts, policy may force TOTP or push; for
**personal** accounts all four are available.

> **Expert sidebar.** The CLI and daemon speak a length-prefixed
> framed-JSON protocol over a local socket. Login is a small
> state-machine: `Request::PasswordSubmission` → (optional
> `Request::SendTwoFactorSms` / `Request::SendTwoFactorNotification`)
> → `Request::SubmitTwoFactorCode` or `Request::SubmitRecoveryCode`.
> The daemon owns the challenge token — the CLI is pure I/O glue.
> This matters when writing a custom wrapper: you re-use the
> running daemon by calling `submit-password`, `submit-tfa`, or
> `submit-recovery` against an existing session rather than
> re-running `login`.

## Step-by-step

### 1. Start the daemon

Pick **one** path per host. Do not start the daemon with `sudo`.

```bash
# Linux — systemd (recommended)
systemctl --user start pcloudd
systemctl --user enable pcloudd           # optional: start on login
systemctl --user status pcloudd           # verify "active (running)"

# macOS — launchd (after a local source/package build)
launchctl kickstart -k gui/$(id -u)/dev.pcloud-rs.daemon

# Windows — per-user daemon (same SID as IPC/DPAPI/WinFSP)
pcloudc start

# Any platform — foreground (useful for debugging; Ctrl-C to stop)
pcloudd serve

# Any platform — friendly wrapper (spawn detached, logs to state dir)
pcloudc start
```

What each command does:

- `systemctl --user start pcloudd` — tells systemd's user
  manager to activate the unit. Exit code 0 means active; a non-zero
  exit or `status: failed` means read the journal:
  `journalctl --user -u pcloudd -n 50`.
- `pcloudc start` — the cross-platform fallback. Spawns `pcloudd serve`
  under the current user, redirects stdio to the platform data directory,
  and returns once an authenticated IPC health request succeeds. `pcloudc
  login` can offer the same startup when it finds no running daemon.

Common failures and fixes:

- `Failed to start pcloud-daemon.service: Unit not found` — the user
  unit isn't installed. Run `systemctl --user daemon-reload` after
  the package installs; if still missing, the package dropped the
  unit in the wrong prefix — re-install.
- `error: socket runtime dir owned by uid 0` — classic `sudo` mistake.
  Delete the runtime dir as root, then run again as your user:
  `sudo rm -rf ~/.local/state/pcloud-rs && pcloudc start`.

### 2. Verify the daemon is up

```bash
pcloudc doctor
# socket:  ~/.local/state/pcloud-rs/ipc.sock (0600, peer-UID ok)
```

The `socket:` line is the one that matters here. Anything other than
`(0600, peer-UID ok)` — stop, do not proceed. See
[Socket and permission errors](#socket-and-permission-errors) below.

> **Expert tip.** On a fleet, use `pcloudc doctor --json --strict`
> and pipe into Prometheus via `node_exporter` textfile collector.
> A dashboard panel on `probes{name="socket",status="ok"}` catches
> misconfigured hosts inside of one scrape interval.

### 3. Interactive login

```bash
pcloudc login -u you@example.com
```

Prompt sequence:

```
Email: you@example.com                  # pre-filled by -u
Password: ********                       # no echo, never logged
Two-factor code (or 'sms', 'push', 'recovery'): 123456
Logged in as you@example.com (uid 12345678).
```

What's happening at each prompt:

- **Email / username** — the address on your pCloud account. Pre-fill
  with `-u` / `--user` / `--username` (all three spellings are
  accepted).
- **Password** — read through the platform's secure password API
  (`termios` `ECHO` off on Unix; `ReadConsole` on Windows). Never
  echoed, never written to shell history, never persisted to disk.
- **Two-factor code** — 6-digit TOTP. Fallback keywords:
  - `sms` — request an SMS code, then re-prompt for it.
  - `push` — send a device notification; approve in the pCloud app,
    then press Enter.
  - `recovery` — switch to the recovery-code flow (one-time codes
    issued when you enabled 2FA).

The CLI sends `Request::PasswordSubmission`, then — if needed — one
of `Request::SubmitTwoFactorCode`, `Request::SendTwoFactorSms`,
`Request::SendTwoFactorNotification`, or `Request::SubmitRecoveryCode`.
The daemon owns the challenge state.

#### Useful login flags (verified against `canonical_token_for`)

| Flag | Purpose |
|---|---|
| `-u`, `--user`, `--username EMAIL` | Pre-fill email. |
| `-m`, `--mountpoint PATH` | Also attach a mount after login (Linux only today; see mount step). |
| `-c`, `--crypto` | Unlock the crypto subsystem on the same session. |
| `--tfa-code CODE` | Supply a TOTP code non-interactively. |
| `-T`, `--tfa-channel sms\|push\|recovery` | Pick the 2FA fallback method. |
| `-r`, `--trust-device`, `--trusted-device` | Ask pCloud to skip 2FA for this device next time. |
| `-s`, `--save-password` | Persist the **auth token** (requires `PCLOUD_DURABLE_AUTH_TOKENS=1`). |
| `--password-stdin` | Read password from stdin (one line, newline-terminated). |
| `--password-env VAR` | Read password from env var `VAR`. |
| `--fuse-opts`, `-O OPTS` | Mount options, passed through to the FUSE layer. |
| `--cache-size GB` | Page-cache cap, in gigabytes. |

> **Expert tip.** `--trust-device` is the single highest-impact
> convenience flag for your own workstation. For shared / build hosts,
> **never** pass it — trusted-device state lives server-side and
> survives re-imaging.

### 4. Non-interactive login (CI, scripts, Ansible)

Three patterns, ordered from **most to least preferred**:

#### 4a. Password from stdin

```bash
printf '%s' "$PCLOUD_PASSWORD" | pcloudc login \
  --user you@example.com \
  --password-stdin \
  --tfa-code "$PCLOUD_TOTP"
```

- `--password-stdin` reads exactly one line, strips the trailing
  newline, and never touches `argv`.
- The secret never appears in `ps`, in audit logs, in error traces,
  or in `/proc/<pid>/cmdline`.
- This is the pattern to use in GitHub Actions, GitLab CI, Jenkins,
  Ansible, anywhere.

Expected output (scripted-friendly):

```
Logged in as you@example.com (uid 12345678).
```

Exit code zero means success. Parse with:

```bash
pcloudc login --json --user … --password-stdin --tfa-code … | \
  jq -r '.uid'
```

#### 4b. Password from a named env var

```bash
export PCLOUD_PW="$(cat /run/secrets/pcloud.pw)"
pcloudc login \
  --user you@example.com \
  --password-env PCLOUD_PW \
  --tfa-code "$PCLOUD_TOTP"
unset PCLOUD_PW
```

`--password-env VAR` reads env `VAR` once, copies its bytes into a
zeroising `SecretString`, and clears the variable. Prefer
`--password-stdin` when you have a choice — env vars are visible to
every child process and to anyone who can read
`/proc/<pid>/environ`.

#### 4c. Durable token vault

```bash
# ONE interactive login with both opt-ins:
PCLOUD_DURABLE_AUTH_TOKENS=1 pcloudc login --save-password

# then:
pcloudc auth-save                         # persist current in-memory token
ls -la ~/.local/state/pcloud-rs/vault.toml
# -rw------- 1 you you 312 ... vault.toml
```

Both opt-ins are required. The vault is written to
`~/.local/state/pcloud-rs/vault.toml` mode `0600` inside a `0700`
parent, with ownership and mode re-validated on every read. On next
invocation, `pcloudc` uses the cached **token** (not the password)
and skips prompts entirely.

> **We do not mirror the C client's raw-password-on-disk behaviour.**
> If you need the actual password available to another process, use
> a proper secret manager — HashiCorp Vault, 1Password, AWS Secrets
> Manager, macOS Keychain, libsecret — and feed it into pCloud via
> `--password-stdin`.

### 5. 2FA: which method when

| Account type | Best first choice | Fallback | When to pick which |
|---|---|---|---|
| Personal, has phone | TOTP | Push | TOTP: offline-safe. Push: fastest on a good network. |
| Personal, phone only | SMS | Recovery | SMS is slow and SIM-swap-prone but usable. |
| Business / Team | TOTP or Push (per policy) | Recovery | Your admin sets the allowed set. |
| Recovery scenario | Recovery code | — | One-time. The code is then invalid; regenerate codes afterwards. |

Keyword fallbacks at the interactive prompt:

- `sms` → `pcloudc send-tfa-sms` is sent under the hood, then the
  prompt re-appears expecting the texted code.
- `push` → `pcloudc send-tfa-notification`; approve in the pCloud
  mobile app, then press Enter.
- `recovery` → switches to `pcloudc submit-recovery` challenge.

Scripted equivalents:

```bash
pcloudc send-tfa-sms
pcloudc submit-tfa 123456              # `tfa 123456` is the legacy alias

pcloudc send-tfa-notification
# approve in app, then:
pcloudc submit-tfa 123456              # the notification returns the code in-app

pcloudc submit-recovery abcd-1234-efgh
```

> **Expert tip (incident response).** Recovery codes are single-use
> and invalidate the whole set once exhausted. Treat them like
> root-CA paper material: store in the same safe you keep your
> KMS master-key printout. After you burn one, regenerate the set
> via the pCloud web UI immediately; otherwise the next on-call may
> have no fallback left.

### 6. Verify login succeeded

```bash
pcloudc status
# authenticated as you@example.com (uid 12345678)
# quota: 142.3 GiB / 2.0 TiB (7%)
# sync roots: 0
```

Machine-readable variants:

```bash
pcloudc status --json | jq '.session'
# {
#   "authenticated": true,
#   "user": {"email": "you@example.com", "uid": 12345678},
#   "quota_bytes_used": 152753849987,
#   "quota_bytes_total": 2199023255552
# }

pcloudc userinfo --json | jq '.email,.cryptosetup,.business'
```

Extract specific fields:

```bash
pcloudc status --json | jq -r '.session.user.uid'       # → 12345678
pcloudc status --json | jq -r '.session.authenticated'  # → true
```

### 7. Log out

```bash
pcloudc logout           # forget in-memory session; keep vault
pcloudc logout --purge   # also zero-overwrite and unlink vault.toml
```

`--purge` overwrites `vault.toml` with zero bytes before unlinking —
best-effort secure deletion. On journalling filesystems (ext4, XFS,
APFS, NTFS) blocks may remain recoverable forensically; assume the
token is compromised the moment the disk leaves your control and
rotate it server-side via the pCloud web UI.

## Verification checklist

- [ ] `pcloudc doctor` prints `socket: … (0600, peer-UID ok)`.
- [ ] `pcloudc status --json | jq -r '.session.authenticated'` → `true`.
- [ ] `pcloudc userinfo --json | jq -r '.email'` matches the email you
      passed to `pcloudc login`.
- [ ] If you opted into the vault: `stat -c '%a' ~/.local/state/pcloud-rs/vault.toml`
      → `600`.
- [ ] `pcloudc logout && pcloudc status --json | jq -r '.session.authenticated'`
      → `false`.

## Troubleshooting — top five

Every login error has a stable exit code. Full table in
[Exit codes](../reference/exit-codes.md).

### 1. `EXIT_AUTH_INVALID_CREDENTIALS` (2) — `Login failed: invalid username or password`

Email (not display name). Caps lock. Password just rotated — wait a
few seconds for propagation. Three bad tries in a row triggers
server-side rate limiting; back off for 60 seconds.

### 2. `EXIT_AUTH_TFA_INVALID` (4) — `Two-factor code invalid or expired`

Almost always clock drift. TOTP codes rotate every 30 seconds; a
clock skew above ~1 minute breaks matching forever. Fix:

```bash
sudo timedatectl set-ntp true             # Linux
sudo sntp -sS time.apple.com              # macOS
w32tm /resync                             # Windows (admin PowerShell)
```

If the clock is fine, check that the authenticator entry you are
reading is labelled with the right pCloud account (email-level,
not just "pCloud").

### 3. `EXIT_NETWORK_UNREACHABLE` (10) — `Cannot reach api.pcloud.com`

The daemon talks TLS to `api.pcloud.com` (US region) or
`eapi.pcloud.com` (EU region). Check:

- outbound 443 not blocked by host / cloud firewall,
- corporate MITM proxy configured under `[network.proxy]` in
  `config.toml`,
- DNS resolves: `getent hosts api.pcloud.com`,
- no captive portal hijacking TLS (hotel / conference Wi-Fi).

### 4. `EXIT_DAEMON_NOT_RUNNING` (20) — `Cannot connect to pcloud-daemon ...`

Start the daemon. If it is running and you still see this, you and
the daemon disagree on `XDG_STATE_HOME` / `HOME`:

```bash
echo "client HOME=$HOME XDG_STATE_HOME=${XDG_STATE_HOME:-$HOME/.local/state}"
ps -o user=,cmd= -C pcloud-daemon
```

Mixed `sudo` invocations are the usual culprit.

### 5. Socket and permission errors

```
Refusing to connect: socket mode is 0666, expected 0600.
```

Stop the daemon, delete the runtime dir, restart:

```bash
systemctl --user stop pcloudd
rm -rf ~/.local/state/pcloud-rs
systemctl --user start pcloudd
```

```
Refusing to connect: peer UID 0 != expected 1000.
```

You ran the CLI with `sudo`. Don't. The daemon runs as you; the CLI
must run as you; no root is ever required.

## Next steps

- [First sync](first-sync.md) — register your first local ↔ remote
  pairing.
- [Mount a virtual drive](first-sync.md#5-first-mount-linux) — uses FUSE,
  fuse-t, or WinFSP according to the qualified native target.
- [Create a public link](first-sync.md#6-first-public-link) — share
  one file without granting account access.
- [Back up a directory](first-sync.md#7-first-backup-snapshot) —
  encrypted, deduplicated snapshot.
- [Exit codes reference](../reference/exit-codes.md).
- [IPC protocol reference](../reference/ipc-protocol.md) — if you are
  writing a custom wrapper around the daemon.
