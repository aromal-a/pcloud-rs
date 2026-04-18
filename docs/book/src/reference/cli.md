# CLI Reference

> **Status: pre-alpha.** Every command, flag, JSON shape, and exit code on
> this page is derived directly from the Rust source
> (`crates/pcloud-cli/src/{app.rs, commands.rs, globals.rs, exit_code.rs,
> field_selector.rs, json_output.rs}`). Surfaces marked **Unavailable**
> parse but the daemon answers `6 Unavailable` until backend gating
> clears. Cross-reference every claim against the code before relying on
> it in automation.

This is the canonical, browser-oriented reference for the `pcloudc`
binary. For the concise manpage form, see `pcloudc(1)`. For the wire
format, see [IPC Protocol](./ipc-protocol.md). For configuration, see
[Configuration](./config.md). For exit-code semantics in isolation, see
[Exit Codes](./exit-codes.md).

## Contents

- [1. Overview](#1-overview)
- [2. Invocation](#2-invocation)
- [3. Global Options](#3-global-options)
  - [3.1 `--json` / `--output`](#31---json-----output)
  - [3.2 `-q` / `--quiet`](#32--q-----quiet)
  - [3.3 `-v` / `-vv` / `-vvv` / `--verbose` / `--debug`](#33--v--vv--vvv---verbose---debug)
  - [3.4 `--help` / `-h`](#34---help---h)
  - [3.5 `--version` / `-V`](#35---version---v)
  - [3.6 `--trace-id <HEX>` (+ `TRACEPARENT`)](#36---trace-id-hex---traceparent)
  - [3.7 `--field` / `-f` / `--select`](#37---field---f---select)
- [4. Command Reference](#4-command-reference)
  - [4.1 Authentication](#41-authentication)
  - [4.2 Session & Diagnostics](#42-session--diagnostics)
  - [4.3 User Info](#43-user-info)
  - [4.4 Sync Roots](#44-sync-roots)
  - [4.5 Mount (FUSE)](#45-mount-fuse)
  - [4.6 Filesystem Status](#46-filesystem-status)
  - [4.7 Public Links](#47-public-links)
  - [4.8 Upload Links](#48-upload-links)
  - [4.9 Shares, Contacts, Teams](#49-shares-contacts-teams)
  - [4.10 Backup Snapshots](#410-backup-snapshots)
  - [4.11 Crypto](#411-crypto)
  - [4.12 Integrity Sweeper](#412-integrity-sweeper)
  - [4.13 `verify`](#413-verify)
  - [4.14 Revision History (`log` / `diff` / `restore`)](#414-revision-history-log--diff--restore)
  - [4.15 Folders](#415-folders)
  - [4.16 Notifications](#416-notifications)
  - [4.17 Audit](#417-audit)
  - [4.18 Daemon Lifecycle](#418-daemon-lifecycle)
  - [4.19 `doctor`](#419-doctor)
  - [4.20 `migrate-from-c`](#420-migrate-from-c)
  - [4.21 `completion`](#421-completion)
  - [4.22 Upload Sessions](#422-upload-sessions)
  - [4.23 `help`](#423-help)
  - [4.24 Account Management](#424-account-management)
  - [4.25 Downloads](#425-downloads)
- [5. Field Selection](#5-field-selection)
- [6. Exit Codes](#6-exit-codes)
- [7. Environment Variables](#7-environment-variables)
- [8. Configuration Integration](#8-configuration-integration)
- [9. Observability & Tracing](#9-observability--tracing)
- [10. Scripting Patterns](#10-scripting-patterns)
- [11. Versioning Policy](#11-versioning-policy)
- [12. See Also](#12-see-also)

---

## 1. Overview

### Beginner intro

`pcloudc` is the command-line front door to the pCloud client. It talks
to a long-running process, `pcloud-daemon`, over a local-only IPC socket
(UNIX socket on POSIX, named pipe on Windows). You run short commands —
"log in", "show status", "create this public link" — and the daemon does
the heavy lifting (authenticated HTTP, sync engine, FUSE mount, crypto
state, audit chain).

| You want to… | Use |
| --- | --- |
| Run a one-shot operation, script it, or wire it into CI | `pcloudc` |
| Keep a persistent session, sync, and mount running | `pcloud-daemon serve` (or `pcloudc start`) |
| Embed pCloud calls inside your own Rust program | the `pcloud-sdk` crate |
| Drive the daemon directly from another language | the IPC protocol ([ipc-protocol.md](./ipc-protocol.md)) |

### Under the hood

- Every subcommand maps to exactly one `pcloud_ipc::methods::Request`
  variant in `commands.rs`.
- The daemon returns a `Response { status, message }` pair, and
  `ExitCode::from_response_status` (`exit_code.rs`) translates the
  status into a numeric process exit.
- `--json` never serialises request-side secrets; the envelope it
  produces is defined in `json_output.rs` and is part of the semver
  surface.
- The only tokens the parser accepts are the ones listed in
  `canonical_token_for` + the `normalize_args` two-word forms. If it
  is not documented here, it does not exist.

---

## 2. Invocation

```
pcloudc [GLOBAL OPTIONS] <COMMAND> [SUBCOMMAND] [ARGS...]
```

- Global options are extracted first, by `GlobalFlags::extract`
  (`globals.rs`). They may appear in **any** position before or after
  the subcommand; however, they are **stripped** from the argv before
  the subcommand parser runs, so their position does not affect how
  positional arguments are counted.
- When `pcloudc` is invoked with **no arguments**, it runs as
  `status` (default route, see `normalize_args`).
- Unknown `--flag` / `-flag` tokens cause `GlobalFlagError::UnknownFlag`
  and exit `2 Usage`. A typo such as `--qiet` never silently falls
  through.
- A bare `-` on the command line is treated as a positional token
  (stdin marker in many tools) and is passed through untouched.

---

## 3. Global Options

| Flag | One-line summary |
| --- | --- |
| [`--json`](#31---json-----output) | Emit a stable JSON envelope on stdout. |
| [`--output text\|json`](#31---json-----output) | Same as above; also accepts `text` / `human` for the default. |
| [`-q`, `--quiet`](#32--q-----quiet) | Suppress stdout. Exit codes survive. |
| [`-v` / `-vv` / `-vvv` / `--verbose`](#33--v--vv--vvv---verbose---debug) | Additive tracing verbosity, capped at 3. |
| [`--debug`, `--dbg`](#33--v--vv--vvv---verbose---debug) | Convenience alias for `-vvv`. |
| [`--help`, `-h`](#34---help---h) | Print help text. |
| [`--version`, `-V`](#35---version---v) | Print version. |
| [`--trace-id <HEX>`](#36---trace-id-hex---traceparent) | Inject a W3C trace id and force-sample this invocation. |
| [`--field <PATH>` / `-f` / `--select`](#37---field---f---select) | Project one or more dotted paths out of the response. |

### 3.1 `--json` / `--output`

**Syntax.** `--json`, `--output json`, `--output=json`, `--output text`,
`--output human`.

**Purpose.** Switches the renderer from free-form human text to a
machine-readable envelope. `--json` is shorthand for `--output json`.
The Rust source of truth is `JsonEnvelope` in `json_output.rs`.

**Envelope shapes (exactly three):**

```json
{"kind":"success","command":"status","status":"ok","message":"daemon is healthy","exit_code":0}
```

```json
{"kind":"error","command":"sync-add","exit_code":7,"error":{"category":"conflict","detail":"duplicate sync root"}}
```

```json
{"kind":"filtered","command":"userinfo","status":"ok","fields":{"quota":10737418240,"premium":false},"exit_code":0}
```

**Examples.**

Beginner:
```bash
pcloudc --json status
```

FAANG-ops (CI gate, fails any non-zero):
```bash
if ! out="$(pcloudc --json status)"; then
  echo "daemon down" >&2; exit 1
fi
printf '%s\n' "$out"
```

**Interactions.** Implied by any use of `--field` / `-f` / `--select`
at the render stage. Suppressed by `--quiet` (envelope still built, not
printed; exit code preserved).

**Honest limitations.** The `message` field of a `success` envelope is
the verbatim daemon payload. For surfaces still emitting
`Debug`-derived strings (userinfo legacy shape, a handful of sync
commands), the message is a flat `key=value, key=value` string —
parseable with `field_selector::parse_message_to_json` but not a real
JSON object. Adding a third output format (`yaml`, `tsv`) would be a
breaking change and is deliberately deferred.

### 3.2 `-q` / `--quiet`

**Syntax.** `-q` or `--quiet`.

**Purpose.** Silences stdout. Stderr is still used for the trace-id
echo (see §3.6) and for argv-password deprecation warnings. Exit codes
are **not** touched.

**Examples.**
```bash
# Cron-safe liveness probe — prints nothing on success, non-zero on failure.
pcloudc -q status || notify-ops "pcloud down"
```

```bash
# Idempotent unmount.
pcloudc --quiet unmount || true
```

**Interactions.** With `--json`, stdout stays empty but the envelope is
still constructed and the exit code is set. Do not use `--quiet` when
you depend on `--json` output downstream.

**Honest limitations.** There is no "quiet stderr" switch. Warnings
about insecure argv passwords are always emitted.

### 3.3 `-v` / `-vv` / `-vvv` / `--verbose` / `--debug`

**Syntax.** `-v` (stackable), `-vv`, `-vvv`, `--verbose` (stacks with
`-v`), `--debug`, `--dbg`.

**Purpose.** Additive hint for the tracing subscriber. `GlobalFlags::
tracing_level` maps verbosity 0-3 to `warn` / `info` / `debug` /
`trace`. `--debug` and `--dbg` jump straight to level 3. Counts above
3 are clamped.

**Examples.**

Beginner:
```bash
pcloudc -v status                # info-level traces
```

FAANG-ops:
```bash
PCLOUD_LOG_LEVEL=trace pcloudc --debug --trace-id 4bf92f3577b34da6a3ce929d0e0e4736 sync-add /mnt/data 7777777
```

**Interactions.** The flag only sets the *hint*; the actual subscriber
is wired in `main.rs` and honours `PCLOUD_LOG_LEVEL` as well. Both
knobs compose (the CLI takes the max).

### 3.4 `--help` / `-h`

**Syntax.** `--help`, `-h`, `help`, `?`. All four route to the same
help renderer.

**Purpose.** Print the top-level help screen. `--json help` emits a
structured success envelope instead of text, so `pcloudc --json --help`
does not choke parsers.

### 3.5 `--version` / `-V`

Prints the semver-tagged crate version and exits `0`. No daemon round
trip.

### 3.6 `--trace-id <HEX>` (+ `TRACEPARENT`)

**Syntax.** `--trace-id <32-lowercase-hex>`, `--trace-id=<HEX>`.

**Purpose.** Inject a W3C Trace Context root trace id and
**force-sample** this invocation regardless of the daemon's head
sampling rate. The CLI synthesises a fresh 16-byte span id and sets
the sampled flags byte (`01`), producing a canonical `traceparent`
of shape `00-<trace>-<span>-01`.

**Precedence** (see `GlobalFlags::traceparent`):

1. `--trace-id` on the CLI (force-samples, always wins).
2. `TRACEPARENT` environment variable, adopted **verbatim** when it
   matches `00-<32hex>-<16hex>-<2hex>` and has non-zero trace+span.
3. Otherwise `None` — the daemon owns the sampling decision.

The chosen traceparent is echoed **once to stderr**:

```
[trace: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01]
```

**Examples.**

Beginner:
```bash
pcloudc --trace-id 4bf92f3577b34da6a3ce929d0e0e4736 status
```

FAANG-ops (correlate a customer ticket across CLI and daemon logs):
```bash
TP=$(uuidgen | tr -d - | tr A-F a-f)
pcloudc --trace-id "$TP" sync-list
journalctl -u pcloudd --since "5m ago" | grep "$TP"
```

**Honest limitations.** Span id is **not** cryptographic — a
nanosecond clock XOR'd with a stack pointer is enough for trace
correlation but not for anything else. Malformed `TRACEPARENT` envvar
values are dropped silently by design so a stray export in the user's
shell does not break every invocation.

### 3.7 `--field` / `-f` / `--select`

**Syntax.**
```
--field <DOTTED_PATH>       (repeatable, order preserved)
-f <DOTTED_PATH>
--select <DOTTED_PATH>
--field=<DOTTED_PATH>       (equals form)
```

**Purpose.** Project one or more whitelisted dotted paths out of the
daemon's response `message`, so shell scripts do not need `jq`. The
parser understands the three message shapes covered in
`field_selector::parse_message_to_json`:

1. real JSON objects/arrays,
2. legacy flat `key=value, key=value` debug strings,
3. everything else — returned as a bare string, so the empty selector
   still works.

See [§5 Field Selection](#5-field-selection) for the full selector
grammar and error semantics.

**Examples.**
```bash
# Get just the quota from userinfo.
pcloudc --field quota userinfo

# Stack multiple selectors; order is preserved in text output.
pcloudc -f quota -f usedquota --select premium userinfo

# Project an array element.
pcloudc --field revisions.0.id log /Documents/Plan.md
```

**Interactions.** When *any* `--field` is supplied, the JSON renderer
emits a `Filtered` envelope (see §3.1) regardless of whether `--json`
was passed; the text renderer prints one value per line in selector
order. A selector that does not match returns exit `2 Usage` and lists
the available sibling keys.

---

## 4. Command Reference

Every token listed below is present in `canonical_token_for` or is an
explicit alias resolved by `parse_single_token` / `normalize_args`. If
a verb is not in this section, it does not parse.

Conventions used for each entry:

- **Synopsis** — argument shape.
- **Purpose / Why** — one-line and short paragraph.
- **Positionals / Flags** — exact layout, adjacency rules.
- **Preconditions** — daemon state, auth, crypto, mount.
- **Output shape** — literal example payloads from tests / runtime.
- **Field-selector tips** — what's safe to project.
- **Examples** — beginner and ops-facing.
- **Failure modes** — common non-zero exits.
- **Related** — cross-links.

### 4.1 Authentication

Canonical tokens: `login`, `logout`, `submit-password` (alias `auth`),
`submit-auth`, `submit-tfa` (alias `tfa`), `submit-recovery`,
`send-tfa-sms`, `send-tfa-notification`, `authsave`.

#### `login`

**Synopsis.**
```
pcloudc login [-u USER] [--password-stdin] [--password-env VAR]
              [--crypto] [-m MOUNT] [--tfa-channel auto|sms|notification]
              [--trust-device] [--save-password]
```

**Purpose.** Begin (or resume) the login REPL driven by the daemon.
Password resolution order, most-secure first:
`--password-stdin` → `--password-env VAR` → argv (deprecated, warns) →
interactive `rpassword` prompt. Argv passwords are zeroised in place.
`--password-env` calls `unsetenv(VAR)` after reading.

**Flags.**

| Flag | Meaning |
| --- | --- |
| `-u`, `--user`, `--username <EMAIL>` | Account email. Defaults to the config's saved username. |
| `--password-stdin` | Read one newline-terminated line from stdin. |
| `--password-env <VAR>` | Read from a named envvar, then `unsetenv`. |
| `--crypto`, `-c` | After login, prompt for the crypto folder passphrase. |
| `-y`, `--passascrypto`, `--pass-as-crypto` | Re-use the account password as the crypto passphrase. |
| `-m`, `--mountpoint <PATH>` | Mount after login. |
| `--tfa-channel <auto\|sms\|notification>` (`-T`, `--channel`) | Default `auto`. |
| `-r`, `--trust-device`, `--trusted-device` | Skip future TFA on this device. |
| `-s`, `--save-password` | **Intentionally does not save passwords** (see auth-vault docs); rejected at runtime with a warning. |

**Output shape.** Text: interactive REPL prompts; JSON: a success
envelope with `command: "login"` on each round-trip.

**Examples.**
```bash
# Beginner: interactive.
pcloudc login -u alice@example.com

# Credential helper supplies the password on stdin.
pass show pcloud/alice | pcloudc login -u alice@example.com --password-stdin

# From cron / CI with an ephemeral envvar.
PCLOUD_PWD="$(vault kv get -field=pwd secret/pcloud/alice)" \
  pcloudc login -u alice@example.com --password-env PCLOUD_PWD
```

**Failure modes.** `2` usage, `3` auth (`Unauthorized`), `4` network,
`5` crypto still locked after `--crypto`, `6` feature unavailable.

**Related.** [`submit-tfa`](#submit-tfa), [`submit-recovery`](#submit-recovery),
[Crypto](#411-crypto).

#### `logout`

Terminates the current session and drops the cached auth token from
the in-memory session state. Durably persisted tokens (vault) are
removed only when `features.durable_auth_tokens_enabled` is `true`.

#### `submit-password` (alias `auth`)

Re-submits the password for the in-flight login REPL state machine.
Argv form prints a stderr warning and zeroises the password in place.

#### `submit-auth`

Submits an already-obtained pCloud auth token to the daemon
(alternative to password-based login). Paired with a credential
helper or a short-lived `PCLOUD_AUTH_TOKEN` envvar.

#### `submit-tfa` (alias `tfa`)

Submits a TFA code for the in-flight login. `tfa <code>` is the
legacy single-token alias.

#### `submit-recovery`

Submits a recovery (breakglass) code.

#### `send-tfa-sms` / `send-tfa-notification`

Re-requests TFA delivery via the named channel. No positional
arguments.

#### `authsave`

Toggles durable auth-token persistence to the owner-only vault at
`$state_dir/auth.vault` (file mode `0600`, parent dir mode `0700`).
Passwords are **never** written — that C behaviour is intentionally
not carried forward.

### 4.2 Session & Diagnostics

#### `status` (alias `st`, default route)

**Purpose.** Daemon-wide health snapshot: uptime, engine counters,
mount state, last-seen transfer activity.

**Output shape (JSON).**
```json
{"kind":"success","command":"status","status":"ok","message":"daemon is healthy; uptime=0h03m; mounts=1; pending=0","exit_code":0}
```

**Field-selector tips.** The status message is a flat descriptive
string today; `--field` will not project fields. Use `--json` and
split on the message text when scripting.

#### `health`

Lightweight probe. When the daemon is built with the `metrics`
feature flag, returns the Prometheus text-format metric-family
snapshot; otherwise returns a small JSON-ish string.

#### `pending` (alias `p`)

Lists in-flight transfer work. Useful for nightly drain loops before
`stop`.

#### `session-status` (two-token alias: `session status`, `session st`)

JSON snapshot: `{expires_at, last_used_at, refresh_in_flight}`. Safe
to project with `-f`.

#### `fs-status` (two-token alias: `fs status <LOCAL-PATH>`, also `filesystem-status`)

Classifies `<LOCAL-PATH>` against the sync engine's view as one of
`INSYNC`, `INPROG`, `NOSYNC`, or `INVSYNC`. Mirrors the C
`psync_filesystem_status` surface.

### 4.3 User Info

#### `userinfo`

**Purpose.** Account summary for the active session.

**Output shape.** Legacy flat form (parseable by the built-in field
selector):
```
userinfo: quota=10737418240, usedquota=3824711, premium=false, email="alice@example.com", cryptosetup=None
```

**Field-selector tips.**
```bash
pcloudc --field quota --field usedquota userinfo
pcloudc --json -f email userinfo
```

### 4.4 Sync Roots

Canonical tokens: `sync-list`, `sync-add`, `sync-remove`,
`sync-change-type`, `sync-localscan`, `sync-suggest`, `sync-is-syncable`,
`pause`, `resume`. Accepted two-token aliases (resolved by
`normalize_args`): `sync list|ls`, `sync add`, `sync remove|rm`,
`sync change-type|set-type|retype`, `sync pause`, `sync resume`,
`sync localscan`, `sync suggest`, `sync is-syncable|syncable`, and the
short alias `s ...` for each.

#### Sync flavors (directions)

Every sync root has a direction flavor. The CLI accepts nine
case-insensitive aliases across three families (mirrors legacy C
`psync_synctype_t`):

| Alias set | Variant | What it does |
|---|---|---|
| `bilateral`, `full`, `both` | `SyncType::Full` | Two-way. Both sides' deletions propagate. Default for `sync add` when `--type` is not supplied. |
| `mirror`, `download-only`, `down`, `remote-to-local` | `SyncType::DownloadOnly` | Remote → local only. Local edits are never uploaded; remote deletions remove the local copy. |
| `upload-only`, `up`, `local-to-remote` | `SyncType::UploadOnly` | Local → remote only. Remote edits are never downloaded. A local deletion **does** propagate to the remote (destructive mirror). |
| `backup`, `backup-archive`, `archive`, `keep-remote` | `SyncType::BackupArchive` | Deletion-safe archival. Uploads new/changed local files like `upload-only`, but a local deletion **does not** delete the remote copy. Rust-only flavor; no legacy C counterpart. |

> **Semantics note.** `backup` is the deletion-safe archival flavor
> (bd-1du.5): uploads new/changed local files but keeps the remote
> copy when the local file is removed. `upload-only` retains the
> legacy destructive-mirror behaviour — use it only when you
> explicitly want a local delete to propagate to the remote. If you
> need a one-shot point-in-time archive (zstd-compressed,
> SHA3-256-stamped tarball) rather than a continuously-replicated
> tree, use `pcloudc backup snapshot-create` instead of a sync root.

Direction can be changed on an existing root without re-adding — see
`sync-change-type` below. A direction change invalidates already-queued
upload/download work because the plan may no longer be valid; the next
scan cycle rebuilds the queue.

#### `sync-list`

Lists persisted sync roots — id, local path, remote path, kind, and
current direction flavor.

#### `sync-add <LOCAL> <REMOTE> [--type FLAVOR]`

Registers a sync root. Local path is canonicalised. Duplicate and
nested local roots are rejected with exit `7 Conflict`. The remote
folder is validated against the backend before the row is persisted.
`--type` selects the direction flavor; unknown aliases exit `2 Usage`
with the full 9-alias list. Default (no `--type`) is bilateral.

Response payload is structured JSON in `message` (ADR-0017):

```json
{"sync_id":7,"local_path":"/home/you/work","remote_path":"/Work","remote_folder_id":1234,"sync_type":"Full"}
```

**Field-selector recipes.**
```bash
pcloudc sync add /l /r --type mirror --field sync_id --field sync_type
# sync_id=7
# sync_type=DownloadOnly

pcloudc --json sync add /l /r --type backup \
  | jq '{id:.result.sync_id, kind:.result.sync_type}'
# {"id":8,"kind":"UploadOnly"}
```

#### `sync-remove <ID>`

Removes the sync root identified by `<ID>`. Queued work for the root
is drained from the engine.

#### `sync-change-type <ID> <FLAVOR>`

Flips the direction of an existing sync root in-place. Accepts the
same 9 `FLAVOR` aliases as `sync add --type`. Mirrors C
`psync_change_synctype`. Preserves the `sync_id`, remote-folder
binding, and staging context; only queued work that no longer matches
the new direction is evicted.

```bash
pcloudc sync change-type 7 mirror          # bilateral -> download-only
pcloudc sync-change-type 7 bilateral       # flip back (canonical token)
```

Failure modes: `InvalidRequest` (non-numeric id or unknown flavor),
`Conflict` (sync id not found), `InternalError` (persistence failure,
rolled back in memory), `Unavailable` (daemon down).

#### `sync-localscan` (aliases `localscan`, `run-localscan`)

Triggers an immediate local-rescan wakeup across all registered
roots. Useful after out-of-band file manipulation.

#### `sync-suggest [<PATH>] [--max N]` (two-token: `sync suggest`)

**Purpose.** Scan the local filesystem under `<PATH>` (default: home
directory) and return a list of candidate folders that could be added
as sync roots. Mirrors C `psync_get_sync_suggestions`. Useful for
onboarding scripts that want to propose sync roots without hard-coding
paths.

**Flags.**

| Flag | Meaning |
| --- | --- |
| `<PATH>` | Positional base path. Defaults to the user's home directory when absent. |
| `--max N` | Hard cap on the number of suggestions returned. No cap by default. |

**Output.** JSON array of suggestion objects in `message`. Each entry
contains at minimum `path` and a classification label.

```bash
pcloudc sync suggest ~/work --max 5
pcloudc --json sync suggest | jq '.[].path'
```

**Related.** `sync-add`, `sync-is-syncable`.

#### `sync-is-syncable <PATH>` (two-token: `sync is-syncable`)

**Purpose.** Classify whether `<PATH>` can be added as a sync root
without conflicting with already-registered roots. Returns `Ok` if the
path is clean, `Conflict` if it duplicates or nests inside an existing
root. Mirrors C `psync_is_folder_syncable`.

```bash
pcloudc sync is-syncable ~/Documents
# exit 0 → safe to add
# exit 7 → already covered by another root
```

**Related.** `sync-suggest`, `sync-add`.

#### `pause` / `resume`

Daemon-wide gate for **all** sync workers. Matches legacy C
`SYNCPAUSE` / `SYNCRESUME`.

### 4.5 Mount (FUSE)

Canonical token: `mount`. `unmount` (alias `umount`). Special form:
`mount --force-umount <PATH>` resolves to `MountForceUnmount`.

#### `mount <PATH>`

Mounts pCloud Drive at `<PATH>` (must exist, be an empty directory,
owned by the current uid, not world-writable). The following FUSE
flags are always applied: `rw,nosuid,nodev,default_permissions`.
`allow_other` / `allow_root` in any custom `--fuse-opts` string are
silently rejected for safety.

**Honest limitations.** Full FUSE runtime parity is tracked under
`bd-1du.4`; behaviour grows until that bead closes.

#### `unmount` (alias `umount`)

Drains pending writes, flushes the journal, releases the kernel
session, and removes stale entries.

#### `mount --force-umount <PATH>`

Escalation path for a daemon-orphaned FUSE session: falls back to
`fusermount -uz` / `umount -f`. Honours `PCLOUD_FORCE_UMOUNT=0` to
refuse the escalation.

### 4.6 Filesystem Status

See [`fs-status`](#fs-status-two-token-alias-fs-status-local-path-also-filesystem-status).

### 4.7 Public Links

All one-token canonical forms:

| Token | Purpose |
| --- | --- |
| `list-links` (alias `list-public-links`) | List all public links. |
| `show-link <CODE>` | Inspect one link. |
| `delete-link <ID>` | Remove one link. |
| `create-file-link <FILEID>` | Create a file link. |
| `create-folder-link <FOLDERID>` | Create a folder link. |
| `change-link-expire <ID> [--expire YYYY-MM-DD]` | Set / clear expiry. |
| `change-link-password <ID>` (stdin) | Set / clear link password. |
| `change-link-upload <ID> <disabled\|any\|registered>` | Change upload policy. |
| `create-tree-link <FOLDERID> [paths...]` | Folder-tree link. |
| `list-link-access <ID>`, `add-link-access <ID> <CONTACT>`, `remove-link-access <ID> <CONTACT>` | Upload-access helpers. |
| `list-bookmarks`, `remove-bookmark <ID>`, `change-bookmark <...>` | Bookmark / pin management. |

Two-token form (supported by `normalize_args`):

```
publink send <CODE> --to <ADDR> [--message <TEXT>] [--from <NAME>]
```

Policy values for `change-link-upload`: `disabled`, `any`, `registered`.

### 4.8 Upload Links

| Token | Purpose |
| --- | --- |
| `create-upload-link <FOLDERID>` | Create a receive-only link. |
| `list-upload-links` | List receive-only links. |
| `delete-upload-link <ID>` | Remove one. |

### 4.9 Shares, Contacts, Teams

Canonical one-token tokens:

- `list-incoming-shares`, `list-outgoing-shares`
- `list-incoming-share-requests`, `list-outgoing-share-requests`
- `list-contacts`, `list-myteams`
- `share-folder <FOLDERID> <EMAIL> <PERMS>`
- `accept-share-request <ID>`, `decline-share-request <ID>`,
  `cancel-share-request <ID>`
- `remove-share <ID>`, `modify-share <ID> <PERMS>`
- `account-stopshare`, `account-modifyshare`, `account-teamshare`
  (business-account share operations)

Crypto-aware variants re-use the `share_temppass` flow transparently.

### 4.10 Backup Snapshots and Backup Management

#### `backup delete <BACKUP_ID>` (two-token: `backup delete`)

**Purpose.** Delete a backup by its remote folder id. Calls
`backup/stopbackup` on the server and removes the matching local sync
root if one is registered. Mirrors C `psync_delete_backup`. Requires
an authenticated session.

**Synopsis.**
```
pcloudc backup delete <BACKUP_ID>
```

`<BACKUP_ID>` is the numeric remote folder id of the backup. If
omitted, the CLI prompts interactively.

**Failure modes.** `3 Unauthorized` (no session), `7 Conflict` (backup
id not found), `6 Unavailable` (network error).

```bash
pcloudc backup delete 9876543
```

#### Backup snapshots

Two-word form (preferred for scripts that already call `backup`):

```
pcloudc backup snapshot-create  [--gpg-recipient EMAIL] [--label STRING]
pcloudc backup snapshot-verify  <PATH_TO_TAR_GPG>
pcloudc backup snapshot-restore <PATH_TO_TAR_GPG> [--yes]
pcloudc backup snapshot-prune   [--retention-days N]
```

Single-token canonical equivalents:
`backup-snapshot-create`, `backup-snapshot-verify`,
`backup-snapshot-restore`, `backup-snapshot-prune`.

**Purpose.** GPG-encrypted, point-in-time snapshot of daemon state
(auth vault + SQLite store via online-backup + audit chain + redacted
config + plugin registry) with verify / restore / prune lifecycle.
The tarball **contains the auth vault** — treat it as equivalent to
the vault itself.

**Preconditions.** Host `gpg(1)` binary MUST be installed; the
recipient key MUST exist in the invoking user's keyring; resolution
failure aborts with exit `7 Conflict`.

**`--yes` on `snapshot-restore`** is required for non-interactive
scripts; without it, stdin must be a TTY or the command aborts with
exit `2 Usage`.

**Recipe: nightly create → verify → prune.** See
[§10 Scripting Patterns](#10-scripting-patterns).

**Related.** [Operations: Backup Snapshots](../operations/backup-snapshots.md).

### 4.11 Crypto

Two-token forms: `crypto <subcommand>` or the short alias `c <subcommand>`.
Single-token canonical forms follow the pattern `crypto-<subcommand>`.

| Two-token form | Single-token canonical | Purpose |
| --- | --- | --- |
| `crypto status` / `c st` | `crypto-status` | Lock state, folder id, fingerprint. |
| `crypto start` | `unlock-crypto` | Unlock the crypto folder. Reads passphrase via stdin / `--password-env` / interactive prompt. |
| `crypto stop` | `lock-crypto` | Lock and zero active key material. |
| `crypto reset` | `crypto-reset` | Wipe local crypto fingerprint and folder registry. Destructive — requires re-setup. |
| `crypto hint` | `crypto-hint` | Fetch the passphrase hint stored at first-time setup. |
| `crypto priv-key-flags` | `crypto-priv-key-flags` | Return current crypto private-key flags as a decimal integer. |
| `crypto send-change-private` | `crypto-send-change-private` | Request a server-side confirmation code authorising a subsequent passphrase rotation. |
| `crypto change-password` | `crypto-change-password` | Rotate the crypto passphrase. Requires the old passphrase, a new passphrase, a hint, and the code from `send-change-private`. |
| `crypto change-password-unlocked` | `crypto-change-password-unlocked` | Rotate the crypto passphrase when the shell is already unlocked (no old passphrase needed). |

#### `crypto start` / `unlock-crypto`

Unlock the crypto folder for this session. Passphrase resolution order:
`--password-stdin` → `--password-env VAR` → argv (deprecated, warns) →
interactive `rpassword` prompt.

#### `crypto stop` / `lock-crypto`

Re-lock the crypto folder and zero in-memory key material immediately.

#### `crypto status` / `crypto-status`

Report whether crypto is locked, unlocked, or uninitialized.

#### `crypto reset` / `crypto-reset`

**Purpose.** Wipe the local crypto fingerprint and folder registry. This
is a recovery operation used when the crypto state has drifted from the
server. After a reset the daemon acts as if crypto was never set up
locally; a full `crypto start` / `unlock-crypto` will re-download and
re-verify the fingerprint from the server.

> **Warning.** This does **not** delete any encrypted data from the
> server. It only clears local bookkeeping. It is still a potentially
> confusing operation — use it only when advised.

#### `crypto hint` / `crypto-hint`

**Purpose.** Fetch the passphrase hint string that was stored when crypto
was first set up. The hint is never the passphrase itself; it is a free-
text reminder chosen by the user at setup time. Requires crypto to be set
up (but does not require it to be unlocked).

```bash
pcloudc crypto hint
```

#### `crypto priv-key-flags` / `crypto-priv-key-flags`

**Purpose.** Return the current value of the `crypto_private_flags` row as
a decimal integer. Mirrors C `psync_crypto_priv_key_flags`. The flags
field encodes metadata such as whether a temporary password is set
(`PSYNC_CRYPTO_FLAG_TEMP_PASS`).

#### `crypto send-change-private` / `crypto-send-change-private`

**Purpose.** Request a server-side one-time confirmation code that
authorises a subsequent `crypto change-password` call. Mirrors C
`psync_crypto_crypto_send_change_user_private`. The code is delivered
out-of-band (e.g. email). Run this first, then run `crypto change-password`
once you have the code.

```bash
pcloudc crypto send-change-private
# (server emails you a code)
pcloudc crypto change-password
# prompts: old passphrase, new passphrase, hint, code
```

#### `crypto change-password` / `crypto-change-password`

**Synopsis.**
```
pcloudc crypto change-password [OLD_PASS [NEW_PASS [HINT [CODE]]]]
```

**Purpose.** Rotate the crypto passphrase. The shell may be locked — the
old password is verified before any change is applied. Requires four
inputs (can be given positionally or prompted interactively):

| Positional | Prompted as | Meaning |
| --- | --- | --- |
| `OLD_PASS` | "Current crypto passphrase" | Current passphrase (validated server-side). |
| `NEW_PASS` | "New crypto passphrase" | Replacement passphrase. |
| `HINT` | (positional arg 3) | Free-text hint stored with the new passphrase. |
| `CODE` | (positional arg 4) | Server-side confirmation code from `send-change-private`. |

Mirrors C `psync_crypto_change_crypto_pass`. Transit-only secrets; the
old and new passphrases are never written to disk.

#### `crypto change-password-unlocked` / `crypto-change-password-unlocked`

**Synopsis.**
```
pcloudc crypto change-password-unlocked [NEW_PASS [HINT [CODE]]]
```

**Purpose.** Same as `crypto change-password` but valid only when the
crypto shell is already unlocked. The old passphrase is not required.
Mirrors C `psync_crypto_change_crypto_pass_unlocked`.

**Honest scope.** `crypto setup` and `crypto setup-folder` remain
intentionally not exposed on the retained CLI path. Use the SDK surfaces
(`pcloud-sdk`) when those flows are needed for first-time setup.

### 4.12 Integrity Sweeper

Two-token form (single-token canonical: `integrity-status`,
`integrity-run-once`, `integrity-skip`):

```
pcloudc integrity              # defaults to `status`
pcloudc integrity status
pcloudc integrity run-once
pcloudc integrity skip <PATH>
```

| Subcommand | Purpose |
| --- | --- |
| `status` | Print the enabled flag, last-run timestamp, per-result-kind counters (`Ok`, `Mismatch`, `LocalMissing`, `RemoteMissing`, `Throttled`, `FetchFailed`), the `audit_drops` counter (audit invariant M1), and the resolved skip-list path. |
| `run-once` | Drive exactly one sweep; honours configured rate limit and skip list. Wired end-to-end today; scheduled automatic runs are still stubbed behind PR2/PR3 walker work. |
| `skip <PATH>` | Append `<PATH>` to the on-disk skip list and hot-reload it in-process. |

Divergence result kinds are mirrored into the `integrity.mismatch`
audit category with a path-HMAC (never cleartext path).

Tracked by `bd-1du.4.6.1`. See
`docs/parity/integrity-sweeper.md`.

### 4.13 `verify`

**Synopsis.**
```
pcloudc verify <PATH> [--recursive] [--fix] [--yes]
```

**Purpose.** Walks the store under `<PATH>`, hashes each local
object, and compares against the server-side hash.

**Output shape (text).** One record per line:
```
[OK]                                 /Documents/plan.md
[MISMATCH local=<hex> server=<hex>]  /Documents/plan.md
[MISSING_LOCAL]                      /Documents/plan.md
[MISSING_REMOTE]                     /Documents/plan.md
```

**Output shape (JSON).** NDJSON — one object per line, not an
enclosing array:
```json
{"kind":"ok","path":"/Documents/plan.md"}
{"kind":"mismatch","path":"/Documents/plan.md","local":"ab…","server":"cd…"}
```

**Flags.**

| Flag | Meaning |
| --- | --- |
| `--recursive` | Descend into every subfolder. |
| `--fix` | Ask the daemon to re-download `MISMATCH` / `MISSING_LOCAL` and re-upload `MISSING_REMOTE`. Destructive; requires `--yes` or an interactive confirmation. |
| `--yes` | Non-interactive confirmation, pairs with `--fix`. |

**Exit codes.**

| Situation | Code |
| --- | --- |
| Every record `[OK]` | `0 Ok` |
| Any `[MISMATCH]` seen (even after `--fix`) | `7 Conflict` |
| Only missing records, no `--fix` | `6 Unavailable` |

**Examples.**
```bash
# Read-only sweep.
pcloudc verify ~/Documents --recursive

# NDJSON for alerting.
pcloudc --json verify ~/Documents --recursive | \
  while IFS= read -r line; do
    case "$line" in *\"kind\":\"mismatch\"*) alert "$line";; esac
  done

# Scripted reconcile (destructive).
pcloudc verify ~/Photos --recursive --fix --yes
```

### 4.14 Revision History (`log` / `diff` / `restore`)

> **Pluggable adapter.** CLI parses; the daemon dispatches through a
> `RevisionProvider`. With no provider configured, the response is a
> structured `{"status":"not_configured",…}` JSON payload and the exit
> code is `6 Unavailable`. Operators can wire an HTTP provider via
> `[file_history].revision_url`. Tracked under `bd-1du.10`.

Canonical tokens: `log` (aliases `file-log`, `file-history`),
`diff` (alias `file-diff`), `restore` (alias `file-restore`).

```
pcloudc log <PATH> [--limit N]
pcloudc diff <PATH> <REV_A> <REV_B>
pcloudc restore <PATH> <REV>
```

**Provider wiring.** The daemon constructs exactly one
`RevisionProvider` per call, selected at runtime from the active
config profile:

| `[file_history].revision_url` | Provider                     | Behaviour                                                                 |
|-------------------------------|------------------------------|---------------------------------------------------------------------------|
| unset (default)               | `NullRevisionProvider`       | Returns `{"status":"not_configured", … "next": …}` + exit `6 Unavailable`. |
| `https://…`                   | `HttpRevisionProvider`       | POSTs `{"path": "<remote path>"}`; expects a JSON array of revisions or `{"revisions":[…]}`. Response is projected to `{"revisions":[…],"count":N}`. |
| `http://…` (Development only) | `HttpRevisionProvider` (test)| Same as above; refused at config-load time in Production profiles.        |

The HTTP provider lives behind the `file-history-http` feature on
`pcloud-proto` so the default build pulls in zero HTTP client code.

**Structured error taxonomy.** When the daemon cannot satisfy a
`log` / `diff` / `restore` call, `Response::message` is a JSON object
with these fields:

| Field     | Kinds                                                                              |
|-----------|------------------------------------------------------------------------------------|
| `status`  | `not_configured` \| `invalid_url` \| `transport` \| `http_status` \| `malformed_response` \| `invalid_request` |
| `message` | Human-readable error text (safe to log).                                           |
| `next`    | Actionable remediation hint naming the exact config key / recovery step.           |
| `path`    | The absolute remote path that triggered the error (audit correlation).             |

Tooling SHOULD key on `status` rather than parse `message`; the set of
values is stable.

Exit-code mapping, CLI flag surface, and the `revision.restored`
audit event are already wired, so enabling the backend requires only
a config change — no CLI release.

### 4.15 Folders

Two-token form (preferred) and single-token aliases:

| Two-word | Single-token aliases |
| --- | --- |
| `folder create <PATH>` | `folder-create`, `create-folder` |
| `folder id <PATH>` | `folder-id`, `get-folder-id` |
| `folder flags <PATH>` | `folder-flags`, `get-folder-flags` |
| `folder owner <PATH>` | `folder-owner`, `get-folder-owner` |

Mirrors C `psync_create_remote_folder_by_path` and siblings. Useful
for driving the pCloud namespace from shell without invoking the
public-link machinery.

### 4.16 Notifications

Canonical: `list-notifications`. Two-token form:

```
pcloudc notifications list              # alias: notif list, notif ls
pcloudc notifications mark-read <UPTO_ID>
```

### 4.17 Audit

```
pcloudc audit verify
```

Verifies the tamper-evident audit chain. Exit `7 Conflict` on chain
break.

### 4.18 Daemon Lifecycle

| Token | Meaning |
| --- | --- |
| `start` (alias `daemon-start`) | Fork `pcloud-daemon serve` with stdout/stderr redirected to `$state_dir/daemon.log` (mode `0600`), detach via `setsid`, poll the socket ~5 s for "daemon listening". Idempotent: responds "already running" and exits `0` if the socket already answers. |
| `finalize` (aliases `shutdown`, `f`, `stop`) | Dispatches `Method::Shutdown`. |
| (implicit) `restart` | Run `stop` then `start` in a script. No single-token `restart` exists. |

### 4.19 `doctor`

Read-only, cross-platform preflight check (`crates/pcloud-cli/src/
doctor.rs`). Returns `0` on all-green, `6 Unavailable` otherwise. With
`--strict`, any non-fatal warning is upgraded to a failure.

**Probe matrix (G6).**

| Probe | POSIX | Windows |
| --- | --- | --- |
| `vault-perms` | file mode `0600`, parent dir `0700`, owner == UID | NTFS-ACL stub: DACL owner-only, no world ACE |
| `disk-free` | `statvfs(cache_dir)` | `GetDiskFreeSpaceExW(cache_dir)` |
| `clock-drift` | NTP/chrony offset, fail if `|drift| > 30 s` | W32Time offset |
| `config-dir-mode` | POSIX bits | NTFS-ACL stub |
| `runtime-dir-mode` | POSIX bits | NTFS-ACL stub |
| `ipc-endpoint` | `$XDG_RUNTIME_DIR/pcloud-rs/daemon.sock` reachable | `\\.\pipe\pcloudd` reachable |
| `daemon-binary` | `pcloud-daemon` on `PATH` | `pcloudd.exe` on `%PATH%` |

`vault-perms` refuses to pass on any platform if another user could
read the vault. Windows ACL validation is currently a stub
(owner-SID check only), tracked under the G6 work stream.

Aliases: `doctor`, `self-check`, `selfcheck`.

### 4.20 `migrate-from-c`

```
pcloudc migrate-from-c [--dry-run] [--force-overwrite] [--from <PATH>]
```

Rehydrates legacy `~/.pcloud/data.db` (C `syncfolder` rows + auth
token blob) into the Rust config/state layout. Dry-run by default;
pass `--apply` (shim for `--force-overwrite`) to commit. Planner and
executor errors are `MigrateError` variants — none carry the legacy
token, which is wrapped in `SecretString` on the happy path. Alias:
`migrate`.

### 4.21 `completion`

```
pcloudc completion <bash|zsh|fish|powershell|elvish>
```

Generates shell completion via the `clap` command tree built in
`completion.rs`.

```bash
pcloudc completion bash > /etc/bash_completion.d/pcloud-rs
pcloudc completion zsh  > "${fpath[1]}/_pcloud-rs"
```

### 4.22 Upload Sessions

Operator-visible upload lifecycle: create, pause, resume, cancel, list.
Sessions live in the daemon's in-memory `SessionRegistry` and are not
persisted across restarts (the crash-safe resume is handled separately
by the upload sidecar journal -- see
[Partial Transfers](../operations/partial-transfers.md)).

#### `upload create`

```bash
pcloudc upload create <LOCAL_PATH> <REMOTE_NAME> [--parent <FOLDER_ID>] \
        [--size <BYTES>] [--conflict-mode <MODE>]
```

Registers a new upload session and returns `{"session_id": <u64>, "remote_name": "<name>", "conflict_mode": "<mode>"}`.

| Flag | Default | Description |
|------|---------|-------------|
| `--parent` | none | Remote parent folder id. |
| `--size` | 0 | Total byte size the upload will stream. |
| `--conflict-mode` | `error` | One of `error`, `overwrite`, `skip`, `rename`. |

**Conflict modes:**

| Mode | Behaviour |
|------|-----------|
| `error` | Refuse if the remote path exists (default, strictest). |
| `overwrite` | Replace the existing remote file. |
| `skip` | Treat existing file as success (no bytes transferred). |
| `rename` | Pick a unique sibling name, e.g. `report (2).pdf`. |

When `rename` is selected the daemon resolves conflicts against the
in-memory registry (sessions under the same `parent_folder_id`).

#### `upload pause`

```bash
pcloudc upload pause <SESSION_ID>
```

Pauses a `Pending` or `InProgress` session. Idempotent against already-paused
sessions. Terminal sessions (`Completed`, `Failed`, `Cancelled`) are rejected
with `Conflict`.

#### `upload resume`

```bash
pcloudc upload resume <SESSION_ID>
```

Resumes a `Paused` session back to `InProgress`. Rejects sessions that are
not currently paused.

#### `upload cancel`

```bash
pcloudc upload cancel <SESSION_ID>
```

Cancels any non-terminal session. Idempotent against already-cancelled
sessions. Terminal `Completed` and `Failed` sessions are rejected.

#### `upload list`

```bash
pcloudc upload list
pcloudc upload ls
pcloudc upload              # bare 'upload' defaults to list
```

Returns a JSON array of all upload sessions known to the running daemon.
Each entry includes `id`, `path`, `remote_name`, `state`, `conflict_mode`,
`offset`, `total_bytes`, `created_at`, `updated_at`, and `history`.

### 4.23 `help`

`help`, `--help`, `-h`, `?` all route to the help command. In `--json`
mode prints a structured success envelope so pipelines never choke on
help text.

### 4.24 Account Management

All account-management commands use the two-token form `account
<subcommand>` or the single-token canonical alias `account-<subcommand>`.

| Two-token form | Single-token alias | Auth required | Purpose |
| --- | --- | --- | --- |
| `account verify-email` | `account-verify-email` | Yes | Trigger a server-side verification email for the active session. |
| `account verify-email-restricted <TOKEN>` | `account-verify-email-restricted` | No | Verify email via an out-of-band verify token (no session needed). |
| `account lost-password <EMAIL>` | `account-lost-password` | No | Send a password-reset email to the given address. Aliases: `account reset-password`, `account forgot-password`. |
| `account change-password` | `account-change-password` | Yes | Change the account password. Prompts for old and new password. |
| `account register <EMAIL>` | `account-register` | No | Register a new pCloud account. Requires `--accept-terms`. |
| `account api-servers` | `account-api-servers` | No | List available pCloud API server regions. |
| `account set-api-server <LOCATION_ID> <BINAPI>` | `account-set-api-server` | No | Pin the daemon to a specific API server region. |
| `account set-language <LANG>` | `account-set-language` | Yes | Set the account language preference (IETF tag, e.g. `en`, `de`). |
| `account promo` | `account-promo` | Yes | Fetch the promotional URL for this platform/locale. |

#### `account verify-email`

**Purpose.** Ask the pCloud server to send a fresh email-verification
message to the address on file for the active session. Use this when a
new account's verification email was lost or expired. Mirrors C
`psync_verify_email`.

```bash
pcloudc account verify-email
```

#### `account verify-email-restricted <TOKEN>`

**Purpose.** Verify the account email address using a `verify_token`
(a short-lived token delivered in the verification email link, not the
session auth token). This variant does not require an active authenticated
session — it can be used from any context that has the token. Mirrors C
`psync_verify_email_restricted`.

```bash
pcloudc account verify-email-restricted eyJhbGciOiJSUzI1NiIs...
```

#### `account lost-password <EMAIL>`

**Purpose.** Send a password-reset email to the specified pCloud account
address. No authentication required — this is a pre-login recovery flow.
Mirrors C `psync_lost_password`.

```bash
pcloudc account lost-password alice@example.com
# Server sends a password-reset link to alice@example.com.
```

**Failure modes.** `7 Conflict` (address not found on pCloud),
`4 Network` (API unreachable), `2 Usage` (missing email argument).

#### `account change-password`

**Synopsis.**
```
pcloudc account change-password [--password-stdin] [--password-env VAR]
```

**Purpose.** Change the pCloud account login password. The daemon prompts
for the current password and a new password, then calls the pCloud API.
A new auth token is returned by the server and installed in the running
session automatically. Mirrors C `psync_change_password`.

Password input follows the same secure-source priority as `login`:
`--password-stdin` → `--password-env VAR` → interactive `rpassword`
prompt. The current password is never written to disk.

```bash
# Fully interactive:
pcloudc account change-password

# Pipe old password from a secret manager:
pass show pcloud/current | pcloudc account change-password --password-stdin
```

**Failure modes.** `3 Unauthorized` (wrong current password),
`7 Conflict` (new password rejected by server policy),
`4 Network`, `2 Usage`.

#### `account register <EMAIL>`

**Synopsis.**
```
pcloudc account register <EMAIL> [--accept-terms]
```

**Purpose.** Register a new pCloud account. No existing session required.
The new account's password is read via the same secure-source priority as
`login`. Terms of Service acceptance is required — pass `--accept-terms`
for non-interactive scripts. Mirrors C `psync_register`.

```bash
pcloudc account register newuser@example.com --accept-terms
```

**Failure modes.** `7 Conflict` (email already registered),
`2 Usage` (terms not accepted), `4 Network`.

#### `account api-servers`

**Purpose.** List the available pCloud API server regions. Returns a JSON
array of `{label, api, binapi, location_id}` objects. No authentication
required. Useful for locating the right `location_id` to pass to
`account set-api-server`.

```bash
pcloudc --json account api-servers | jq '.[] | {id: .location_id, label}'
```

#### `account set-api-server <LOCATION_ID> <BINAPI>`

**Purpose.** Pin the daemon to a specific pCloud API server region. The
`location_id` and `binapi` hostname come from `account api-servers`.
Persists to the store and updates all live protocol transports. Silently
rejected (`InvalidRequest`) when the daemon's `data_residency` policy
does not allow the target region. Mirrors C `psync_set_api_server`.

```bash
# Pin to the US region (find LOCATION_ID from api-servers output):
pcloudc account set-api-server 1 binapi.pcloud.com
```

**Failure modes.** `7 Conflict` (policy rejection), `2 Usage` (invalid
args), `4 Network`.

#### `account set-language <LANG>`

**Purpose.** Set the account language preference on the server. `<LANG>`
is an IETF language tag (e.g. `en`, `de`, `fr`, `es`). Requires an
authenticated session. Mirrors C `psync_set_language`.

```bash
pcloudc account set-language de
```

#### `account promo`

**Purpose.** Fetch the promotional banner URL for the current
platform/locale. Returns a JSON object `{url, width, height}` in
`message`, or the string `"no promo"` when the server has no active
promotion. Requires an authenticated session. Mirrors C `psync_get_promo`.

```bash
pcloudc --json account promo | jq '.message | fromjson | .url'
```

### 4.25 Downloads

Two-token form: `download <subcommand>`. Short alias `dl`. Single-token
canonical forms: `download-link`, `download-file`.

#### `download link <FILE_ID>` (two-token: `download link`)

**Synopsis.**
```
pcloudc download link <FILE_ID>
```

**Purpose.** Resolve the download URL for a remote file by its numeric id.
Returns a JSON object with `hosts`, `path`, and `download_tag` fields.
Requires an authenticated session. Mirrors C `psync_get_file_link` /
the pCloud `getfilelink` API call.

The resolved URL is time-limited and account-specific. It can be passed
to `curl` or `wget` for immediate download.

```bash
# Get the download link for file 987654321:
pcloudc --json download link 987654321 | jq '.message | fromjson | .hosts[0] + .path'
```

**Field-selector recipe.**
```bash
pcloudc --field path download link 987654321
```

**Failure modes.** `3 Unauthorized` (no session), `7 Conflict` (file id
not found), `4 Network`.

#### `download file <FILE_ID> <LOCAL_PATH>` (two-token: `download file`)

**Synopsis.**
```
pcloudc download file <FILE_ID> <LOCAL_PATH>
```

**Purpose.** Download a remote file by its numeric id to a local absolute
path. Internally resolves the download URL via `getfilelink`, then
performs an HTTPS fetch and writes the bytes to `<LOCAL_PATH>`. Requires
an authenticated session.

`<LOCAL_PATH>` must be an absolute path. The file is written atomically
(staged to a temp path in the same directory, then renamed). If the
destination exists, it is overwritten.

```bash
pcloudc download file 987654321 /tmp/downloaded.pdf
```

**Failure modes.** `3 Unauthorized` (no session), `7 Conflict` (file id
not found), `4 Network` (download failed mid-transfer), `2 Usage` (missing
path argument).

---

## 5. Field Selection

### Beginner intro

`--field quota userinfo` is "run `userinfo` and print only the
`quota` key". Multiple `--field` flags stack in the order you pass
them. The selector works on any command; commands whose message is
not parseable into an object just return the empty selector's value.

### Grammar

```
selector    := "." | "" | path
path        := segment ("." segment)*
segment     := key | index
key         := <any run of non-"." characters>
index       := <0-9>+
```

- `"."` and `""` select the whole parsed value.
- A leading `.` is tolerated (`.quota` ≡ `quota`) for jq muscle
  memory.
- A segment is treated as an **array index** only if it parses as
  `usize` end-to-end — `quota.0` picks element `0` of `quota` when
  `quota` is an array, and picks the key `"0"` otherwise.

### Accepted message shapes

The selector runs against
`field_selector::parse_message_to_json(&response.message)`:

1. **Real JSON.** `{"quota":10737418240,"premium":false}` — parsed
   directly.
2. **Legacy flat `key=value, key=value`.** Optionally prefixed with
   `ident: `. Values are inferred as number / bool / string. Useful
   for `userinfo`, `crypto-status`, several sync responses.
3. **Plain text.** Returned as `Value::String`. Only the empty
   selector matches.

### Error semantics

| Error | Exit | Example message |
| --- | --- | --- |
| Key not found on an object | `2 Usage` | `field not found: 'quota.bogus'. available: premium, quota, usedquota` |
| Index out of range | `2 Usage` | `field not found: 'revisions.99.id'. available: [0..5]` |
| Type mismatch (key on array, index on scalar) | `2 Usage` | `type mismatch at 'quota': expected object, got number` |

Error messages **never echo user-supplied values**; they print field
names and sibling keys only.

### Security invariant

The selector only touches `Response::message`. The daemon already
stripped every secret-bearing field upstream, and the secret wrappers
(`SecretString`, `SecretBytes`) do not implement `Serialize` for
their protected payload, so a selector cannot reach into them. This
is pinned by `assert_no_secret_in_value` in `field_selector.rs`.

### JSON vs text rendering

| Mode | Output |
| --- | --- |
| Text, one `--field` | Bare value, no prefix. |
| Text, multiple `--field` | One value per line, in selector order. |
| `--json` | `JsonEnvelope::Filtered { command, status, fields, exit_code }`; `fields` is keyed by the original dotted path. Map ordering is alphabetical — use text mode if you need selector order in JSON. |

---

## 6. Exit Codes

Stable ABI (see `EXIT_CODE_HELP` in `exit_code.rs`).

| Code | Variant | When you will see it |
| --- | --- | --- |
| `0` | `Ok` | Command completed successfully. |
| `1` | `GenericError` | Fallback bucket — classifier couldn't do better. Usually indicates a CLI-internal bug; file a ticket. |
| `2` | `Usage` | Unknown flag / missing arg / selector not found / IPC `InvalidRequest`. First line of triage: re-read `pcloudc --help`. |
| `3` | `Auth` | Bad credentials, expired token, TFA cancel, daemon `Unauthorized`, or transport errors that match `auth*fail*` / `unauthorized`. |
| `4` | `Network` | Socket refused, connect timeout, broken pipe, "no such file" (socket missing), generic transport words. If `doctor` cannot reach the IPC endpoint, you'll get `4` from every downstream command. |
| `5` | `CryptoLocked` | Crypto path locked or unavailable. Common when a sync root lives inside a crypto folder and the user hasn't run `unlock-crypto`. |
| `6` | `Unavailable` | Daemon unreachable, feature disabled, or an endpoint that's wired but gated (`log`, `diff`, `restore` today). Also the canonical `doctor` failure code. |
| `7` | `Conflict` | Duplicate sync root, already-mounted path, GPG key missing, audit chain break, `verify` mismatch, `PolicyViolation`. |
| `8` | `Internal` | Daemon reported `InternalError`. Treat as a bug — collect `daemon.log`, trace id, and `audit verify` output; file a ticket. |

Forward-compat: patch/minor releases MAY add new codes **at the end**
of the enum but MUST NOT change or reuse existing values; removing or
renumbering a code requires a major release.

---

## 7. Environment Variables

### Read by the CLI

| Variable | Purpose |
| --- | --- |
| `PCLOUD_ROOT` | Override the base runtime/state/config root (used for multi-instance setups; all derived paths fan out from here). |
| `PCLOUD_ENV` | Named environment profile (`dev`, `prod`, etc.) — selects a config overlay. |
| `PCLOUD_API_MODE` | Transport policy. Production rejects any plaintext downgrade. |
| `PCLOUD_API_HOST`, `PCLOUD_API_PORT` | API host / port override. Validated; production TLS policy is not relaxed. |
| `PCLOUD_API_SERVER_NAME` | Named API server (friendly alias for host/port pair). |
| `PCLOUD_DURABLE_AUTH_TOKENS` | `1` to opt-in to vault persistence. Defaults off. |
| `PCLOUD_CACHE_SIZE_GB` | FUSE cache budget, in GiB. Takes precedence over `[mount].cache_size_mb`. |
| `PCLOUD_MOUNT_CACHE_SIZE_MB` | Page-cache memory budget in MiB (default 256). |
| `PCLOUD_MOUNT_PAGE_CACHE_ENTRIES` | Max metadata-cache entries (default 4096). |
| `PCLOUD_MOUNT_METADATA_TTL_SECS` | Metadata-cache TTL in seconds (default 60). |
| `PCLOUD_DEFAULT_MOUNTPOINT` | Default `-m` target for `login` / `mount`. |
| `PCLOUD_LOG_PATH` | Daemon log destination (owner-only `0600`). |
| `PCLOUD_FS_EVENT_LOG` | Separate fs-event log path. |
| `PCLOUD_LOG_LEVEL` | Fallback when no `-v` flag is passed (`warn`, `info`, `debug`, `trace`). |
| `PCLOUD_FUSE_OPTS` | Comma-separated FUSE opts; `allow_other` / `allow_root` are silently stripped. |
| `PCLOUD_CONFIG` | Path to `config.toml` when `--config` is not given. |
| `PCLOUD_FORCE_UMOUNT` | Set to `0` to refuse `mount --force-umount` escalation. |
| `TRACEPARENT` | W3C trace context, adopted verbatim when well-formed. Malformed values are silently dropped. See [§3.6](#36---trace-id-hex---traceparent). |

### Rationale

- Every variable is read **once** at startup; `pcloudc` does not
  re-read them between commands (nothing to race).
- `--password-env VAR` calls `unsetenv(VAR)` immediately after reading,
  so the password is invisible to `/proc/self/environ` afterwards.
- Any envvar that contains a path is validated the same way a config
  value is — mode bits, ownership, world-access checks.
- See [Configuration](./config.md) for the full list with default
  values and per-key security notes.

---

## 8. Configuration Integration

Precedence order (highest wins):

1. Flags on the command line (e.g. `--config`, `--log-level`,
   `--cache-size`).
2. Environment variables (see §7).
3. `~/.pcloud/config.toml` (or `$PCLOUD_CONFIG` / `--config <PATH>`).
4. Compiled-in defaults.

`pcloudc` never rewrites `config.toml` automatically — there is no
`config set` surface on the CLI; edit the file and restart the daemon
(or let `authsave` do its scoped write to the auth vault). This is an
intentional divergence from the legacy C client, chosen so a typo in
a live environment cannot persist itself.

---

## 9. Observability & Tracing

- **Single invocation.** Use `--trace-id <HEX>` to force-sample one
  call and correlate across CLI, daemon, and downstream HTTP calls.
- **Distributed context.** Export `TRACEPARENT=...` before running
  `pcloudc` from a traced parent job (most CI runners do this
  automatically).
- **Echo on stderr.** The chosen traceparent is printed once to
  stderr as `[trace: 00-<trace>-<span>-01]` before the result. Copy
  that line into support tickets; it's enough to locate every log
  message for the call.
- **Log correlation.** The daemon includes the same trace id in every
  structured log line, so `journalctl -u pcloudd | grep <id>`
  gives you the full server-side picture.

See the enterprise tracing doc (`docs/book/src/enterprise/tracing.md`)
for exporter configuration and retention.

---

## 10. Scripting Patterns

Ten recipes. All shell-only; no `jq`.

### 10.1 Liveness probe

```bash
pcloudc -q status || systemctl restart pcloudd
```

### 10.2 Token health for CI

```bash
STATUS=$(pcloudc --json --field status session-status)
case "$STATUS" in
  *"\"ok\""*) ;;
  *) echo "session unhealthy"; exit 3;;
esac
```

### 10.3 Bulk link cleanup

```bash
# Delete every public link older than 90 days.
NINETY=$(date -u -d "90 days ago" +%s)
pcloudc --json list-links \
  | while IFS= read -r line; do
      id=$(printf '%s' "$line" | pcloudc --field id --json show-link)
      created=$(printf '%s' "$line" | pcloudc --field created --json show-link)
      [ "$created" -lt "$NINETY" ] && pcloudc delete-link "$id"
    done
```

### 10.4 Nightly backup via cron

```cron
15 2 * * *  pcloud-rs  ( \
  pcloudc backup snapshot-create \
      --gpg-recipient dr-team@example.com \
      --label "nightly-$(date -u +\%F)" \
 && LATEST=$(ls -t /var/backups/pcloud-rs/*.tar.gpg | head -1) \
 && pcloudc backup snapshot-verify "$LATEST" \
 && pcloudc backup snapshot-prune --retention-days 14 \
 ) >> /var/log/pcloud-rs/backup.log 2>&1
```

### 10.5 Integrity sweeper scheduler

```bash
# /etc/systemd/system/pcloud-integrity.timer → OnCalendar=*-*-* 03:30:00
pcloudc integrity run-once
EXIT=$?
if [ "$EXIT" -eq 7 ]; then
  alert "pcloud integrity mismatch"
fi
```

### 10.6 Scripted sync reconcile

```bash
pcloudc verify "$SYNC_ROOT" --recursive > /tmp/verify.log
if grep -q '^\[MISMATCH' /tmp/verify.log; then
  pcloudc verify "$SYNC_ROOT" --recursive --fix --yes
fi
```

### 10.7 Audit chain watchdog

```bash
pcloudc --quiet audit verify
[ $? -eq 7 ] && page-oncall "pcloud audit chain break"
```

### 10.8 Quota headroom alert

```bash
Q=$(pcloudc --field quota userinfo)
U=$(pcloudc --field usedquota userinfo)
HEADROOM=$(( (Q - U) * 100 / Q ))
[ "$HEADROOM" -lt 10 ] && alert "pcloud <10% free"
```

### 10.9 Force-umount stuck mount

```bash
if ! mountpoint -q /mnt/pcloud; then
  PCLOUD_FORCE_UMOUNT=1 pcloudc mount --force-umount /mnt/pcloud
fi
```

### 10.10 Correlation-id fanout

```bash
TID=$(python3 -c 'import secrets; print(secrets.token_hex(16))')
pcloudc --trace-id "$TID" sync-add "$L" "$R"
pcloudc --trace-id "$TID" sync-list
journalctl -u pcloudd --since "-5m" | grep "$TID"
```

---

## 11. Versioning Policy

- **Flag names** (§3 and `known_flag_names`) are additive under
  semver. Removing or renaming any global flag requires a major
  release with a changelog migration note.
- **Command tokens** (`canonical_token_for`) are additive. Legacy
  aliases resolved by `normalize_args` are preserved as long as they
  have shipped in a stable release.
- **Exit codes** (§6) follow the stable-ABI guarantee in
  `ExitCode`: values may be added at the end; existing values never
  change or get reused.
- **JSON envelope** (`JsonEnvelope`) is stable; new fields may be
  added, existing fields never change shape.
- **`--json` output format** is the only machine-readable contract;
  text output is stable for readability but **not** parse-stable.
  Scripts should always pass `--json` (and optionally `--field`).

**Pre-alpha caveat.** Until `bd-1du.10` closes and the project
declares beta, any of the above may tighten **only** in the
restricting direction (more validation, stricter rejection). No
silent relaxations.

---

## 12. See Also

- [Configuration](./config.md) — `config.toml` schema, all `PCLOUD_*`
  variables, mode-bit and ownership invariants.
- [IPC Protocol](./ipc-protocol.md) — wire format for the request the
  CLI actually sends.
- [Exit Codes](./exit-codes.md) — standalone exit-code reference.
- [Operations: Backup Snapshots](../operations/backup-snapshots.md) —
  GPG key management, offsite destinations.
- [Enterprise: Tracing](../enterprise/tracing.md) — trace-id
  propagation, exporter configuration.
- `pcloudc(1)` — manpage (`docs/man/pcloudc.1`).
- C-to-Rust parity matrix: `C_FEATURE_PARITY_MATRIX.csv`.
