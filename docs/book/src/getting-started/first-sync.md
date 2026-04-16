# First Sync, First Mount, First Link, First Backup

> **TL;DR** — the four "hello world" flows, end-to-end:
>
> ```bash
> pcloudc sync-add ~/work /Work                 # first sync pairing
> pcloudc mount /mnt/pcloud                     # first virtual drive (Linux)
> pcloudc create-file-link "/Docs/readme.pdf"   # first public link
> pcloudc backup-snapshot-create ~/important \
>   --gpg-recipient you@domain                  # first encrypted backup
> pcloudc doctor --strict                       # full probe, CI-grade
> ```
>
> All four features share the running daemon; you do not need a mount
> to sync, and you do not need sync to mount. Mount on macOS / Windows
> / BSD is **scaffolded only** today (see `bd-1du.4`). The revision
> history preview (`log` / `diff` / `restore`) returns **Unavailable**
> on purpose — it is CLI-only scaffolding until the parity gate lands.

## What you'll learn

- How the sync engine, the mount engine, the public-link surface, and
  the backup engine interact (and how to reason about them
  independently).
- How to register a first sync root, including remote-folder-id
  resolution with `pcloudc folder-id`.
- How to observe progress with `pcloudc status`, `pcloudc status --watch`,
  and `pcloudc status --json --follow` (structured events).
- How to mount the virtual drive on Linux, and what to expect on
  non-Linux today.
- How to create, inspect, and delete a public file link, extracting
  the id and URL with `jq` field selectors.
- How to take your first encrypted backup snapshot, verify it, and
  restore.
- What each probe in `pcloudc doctor --strict` means for sync, mount,
  TLS policy, and key material.

## Conceptual background

The daemon hosts **four loosely coupled subsystems**, all sharing the
same SQLite state store and the same authenticated transport:

1. **Sync** — file-level bidirectional replication between a local
   directory and a remote pCloud folder. Drives an event-driven
   scanner (inotify / FSEvents / ReadDirectoryChangesW / polling
   fallback), a transfer queue, and a conflict resolver.
2. **Mount** — a FUSE-style virtual filesystem that lets you browse
   the entire remote tree without syncing. Reads are on-demand,
   writes are staged and committed through the transfer queue.
3. **Public links** — create / list / show / delete shareable URLs
   for files or folders, with optional password, expiry, and upload
   allowance. No account-level grant required on the consumer side.
4. **Backup** — snapshot a local directory into a signed,
   GPG-encrypted, deduplicated archive and push it to pCloud. Not a
   sync pairing; snapshots are content-addressed and retained by
   policy.

You pick the subsystems you need. A typical desktop user registers
one sync pairing and a mount on the same host. A backup box registers
only a nightly snapshot. A share-heavy workflow uses nothing but
public links.

> **Expert sidebar.** Every subsystem talks to the daemon through
> the same IPC request dispatch; there is no privileged back-channel.
> The staging area (`~/.local/state/pcloud-rs/staging/`) is where
> mid-transfer data lives — on crash, the daemon re-enumerates
> staging on start and either resumes (idempotent via `upload_create`
> ids) or discards partial byte ranges. You will not see a truncated
> file land in a sync root or a mount point.

## 1. First sync pairing

```bash
pcloudc sync-add ~/work /Work
# Canonicalized local path: /home/you/work
# Remote folder resolved:   /Work (folderid 123456789)
# Sync root added. Initial scan queued.
```

Both token forms work:

- `pcloudc sync-add <LOCAL> <REMOTE>` — kebab form.
- `pcloudc sync add <LOCAL> <REMOTE>` — subcommand form. Rewritten
  to `sync-add` before dispatch; they are identical.

What the daemon does on add:

1. **Canonicalize** the local path (resolve symlinks, normalize
   case on case-insensitive filesystems). `~/work` becomes
   `/home/you/work` (or `C:\Users\you\work`).
2. **Refuse** the add if the path is already a sync root, is
   *inside* an existing root, or *contains* an existing root.
   Nested roots create ambiguous deletion semantics.
3. **Validate the remote folder** — calls `listfolder` under the
   hood. Without `--create-remote`, the add fails if the remote
   folder does not exist.
4. **Persist** the pairing to `~/.local/state/pcloud-rs/store.sqlite3`
   and queue the initial scan. The scan walks the local tree,
   compares to the remote listing, and enqueues uploads / downloads.

### Resolving a folder id before you add

If you want to check the remote folder exists (and grab its numeric
id) before calling `sync-add`:

```bash
pcloudc folder-id /Work
# 123456789

pcloudc folder-id /Work --json | jq -r '.folderid'
# 123456789
```

Exit code is non-zero if the folder doesn't exist.

### Useful `sync-add` flags

| Flag | Effect |
|---|---|
| `--type FLAVOR` | Direction flavor. Nine aliases across three families (see below). Default: bilateral. |
| `--json` | Machine-readable output. Response payload is structured JSON (ADR-0017) including `sync_id`, `sync_type`, `remote_folder_id`. |

On Windows, local paths accept `\` or `/`; remote paths always use `/`.

### Sync flavors: bilateral, mirror, backup

Three direction families, nine case-insensitive aliases:

| You want… | Use `--type` | Maps to | Notes |
|---|---|---|---|
| Two-way reconciliation (default) | `bilateral` / `full` / `both` | `SyncType::Full` | Interactive workstations; local **and** remote deletions propagate. |
| A read-only local replica | `mirror` / `download-only` / `down` / `remote-to-local` | `SyncType::DownloadOnly` | Local edits are never uploaded; a remote deletion removes the local copy. |
| A one-way local-to-remote push | `backup` / `upload-only` / `up` / `local-to-remote` | `SyncType::UploadOnly` | Remote edits are never downloaded. |

```bash
pcloudc sync add ~/work    /Work    --type bilateral       # default
pcloudc sync add ~/Photos  /Photos  --type mirror          # read-only replica
pcloudc sync add ~/archive /Archive --type backup          # push-only (see caveat)
```

> **Honest caveat (pre-alpha).** The `backup` alias currently maps to
> the same `UploadOnly` semantics as `upload-only` and **does**
> propagate local deletions to the remote. A true deletion-safe backup
> flavor is tracked under a new bead
> (`bd-1du.5 Deletion-safe backup sync flavor`). For deletion-safe
> archival today, use `pcloudc backup snapshot-create` — a
> GPG-encrypted tarball that is not a sync root and never deletes.

You can change the direction of an existing sync root without
re-adding:

```bash
pcloudc sync change-type 7 mirror        # flip sync_id=7 to download-only
```

Direction changes are cheap: the `sync_id`, remote-folder binding, and
staging context stay; only queued work that no longer matches the new
plan is evicted. The next scan cycle rebuilds the queue.

## 2. Observing progress

### One-shot

```bash
pcloudc status
# you@example.com, quota: 142.3 GiB / 2.0 TiB (7%)
# sync roots: 1 active
#   /home/you/work <-> /Work
#     state:    scanning
#     queued:   142 uploads, 0 downloads
#     progress: 18 / 142 (12%)
```

Short alias: `pcloudc st`.

### Live watch

```bash
pcloudc status --watch
```

Redraws once per second. `q` or Ctrl-C exits. On Windows the VT
sequences require Windows 10 1809+; older shells fall back to
scrolling.

### Structured events (scripts / dashboards)

```bash
pcloudc status --json --follow
```

One JSON line per event:

```json
{"ts":"2026-04-16T10:30:01Z","kind":"transfer.start","sync":"/home/you/work","path":"notes.md","bytes":4096}
{"ts":"2026-04-16T10:30:01Z","kind":"transfer.progress","path":"notes.md","done":4096,"total":4096}
{"ts":"2026-04-16T10:30:01Z","kind":"transfer.done","path":"notes.md","result":"ok"}
```

Pipe into Prometheus / Loki / Splunk. Schema stable within a major
version — see the IPC protocol reference
([`reference/ipc-protocol.md`](../reference/ipc-protocol.md)) for the
full event enum.

Extract fields:

```bash
pcloudc status --json --follow | \
  jq --unbuffered 'select(.kind=="transfer.done") | {path, result}'
```

> **Expert tip.** For fleet dashboards, the canonical rollup metric
> is `queued_uploads + queued_downloads` per sync root. Alert at
> sustained >1000 queue depth for over 5 minutes — that's the
> threshold where network, disk, or remote quota is usually the
> root cause, not pCloud itself.

## 3. Selective sync via `.pcloudsync`

Drop a `.pcloudsync` at the root of any sync pairing; format is
gitignore-flavoured:

```gitignore
# build artefacts
target/
*.o
*.pyc
**/.DS_Store
node_modules/

# negate: re-include
!target/release/useful-binary

# OS noise
[Tt]humbs.db
.Spotlight-V100/
.Trashes/
```

Rules:

- `#` starts a comment.
- `!` negates a prior match (gitignore semantics).
- `**` matches any number of directory levels.
- Patterns without `/` match at any depth; patterns with `/` are
  anchored to the sync root.
- File is re-read on every scan cycle — no daemon restart needed.
- The file itself is synced by default (for cross-device consistency);
  add `!/.pcloudsync` to keep it local.

## 4. Listing and removing sync pairings

```bash
pcloudc sync-list
# ID  LOCAL                  REMOTE   DIRECTION  STATE       QUEUED
# 1   /home/you/work         /Work    both       idle        0
# 2   /home/you/Pictures     /Photos  up         scanning    421

pcloudc sync-list --json | jq '.[] | {id, local, state, queued}'

pcloudc sync-remove /home/you/work
# Sync root removed. Queued work evicted. Local and remote files untouched.
```

`sync-remove` only forgets the pairing. Add `--delete-local` or
`--delete-remote` to wipe one side; both require `--yes` or an
interactive confirmation.

## 5. First mount (Linux)

```bash
sudo mkdir -p /mnt/pcloud
sudo chown $(id -u):$(id -g) /mnt/pcloud
pcloudc mount /mnt/pcloud
# Mounted pCloud at /mnt/pcloud (fuse3 3.x, cache 5 GiB)
```

What this does:

1. Ensures `/dev/fuse` is accessible and a FUSE provider is
   installed (`fuse3` on Linux, `fuse-t` on macOS).
2. Spawns the mount loop inside the daemon; the daemon now serves
   FUSE requests for `/mnt/pcloud`.
3. Populates the root listing lazily — `ls /mnt/pcloud` triggers a
   first `listfolder` call; reads are paged and cached.

### Honest status

Linux mount is the only wired target today. **macOS**, **Windows
(ProjFS)** and **BSD** mount runtimes are scaffolded behind
`bd-1du.4` — the commands parse and the daemon accepts them, but
you should treat them as pre-alpha until that bead closes. Classic
`sync-add` works on every platform and is the recommended way to
use pCloud non-interactively anywhere mount is not ready.

### Useful mount flags

| Flag | Effect |
|---|---|
| `-m`, `--mountpoint PATH` | Alternative form of the positional `<PATH>`. |
| `-O`, `--fuse-opts OPTS` | Raw FUSE options (e.g. `-o allow_other`). |
| `--cache-size GB` | Page-cache cap in gigabytes (default 5, same default as the C client). |
| `--force-umount` | Recover an orphan mount: kills the existing FUSE session and remounts. |

### Unmount

```bash
pcloudc unmount /mnt/pcloud
# or clean up an orphan:
pcloudc mount --force-umount /mnt/pcloud
```

> **Expert tip.** For read-heavy workloads, bump `--cache-size` to
> 25–50 GiB on hosts with SSDs. The cache is kept in the runtime dir
> (`~/.local/state/pcloud-rs/`), honours the parent mode `0700`, and
> is safe to evict wholesale — just `pcloudc unmount` and
> `rm -rf ~/.local/state/pcloud-rs/page-cache`.

## 6. First public link

Create:

```bash
pcloudc create-file-link "/Docs/readme.pdf"
# https://u.pcloud.link/publink/show?code=xYzAbC12
# link id: 42
```

Machine-readable:

```bash
pcloudc create-file-link "/Docs/readme.pdf" --json
# {
#   "linkid": 42,
#   "code": "xYzAbC12",
#   "shorturl": "https://u.pcloud.link/publink/show?code=xYzAbC12",
#   "expire": null,
#   "password": false,
#   "created": "2026-04-16T10:42:00Z"
# }
```

Field extraction:

```bash
LINK=$(pcloudc create-file-link "/Docs/readme.pdf" --json | jq -r '.shorturl')
ID=$(pcloudc create-file-link "/Docs/readme.pdf" --json | jq -r '.linkid')
echo "$LINK"
echo "$ID"
```

Inspect an existing link:

```bash
pcloudc show-link 42 --json | jq
```

Delete:

```bash
pcloudc delete-link 42
```

Add an expiry or a password after the fact:

```bash
pcloudc change-link-expire   42 2026-12-31
pcloudc change-link-password 42 'some-strong-passphrase'
```

Rules of thumb:

- `change-link-expire <id> <DATE|none>` — ISO-8601 date (or literal
  `none` to clear).
- `change-link-password <id> <PASS|none>` — pass a password (read
  from stdin if you prefer), or `none` to clear.
- `create-tree-link <PATH>` — recursive tree link across a folder
  sub-hierarchy.
- `list-links` / `list-upload-links` — enumerate current links.

> **Expert tip.** For one-off external sharing, prefer
> `create-file-link` with an explicit expiry: `... --json |
> jq -r '.linkid' | xargs -I{} pcloudc change-link-expire {} 2026-05-01`.
> Always pair password + expiry on any link that leaves your
> immediate team.

## 7. First backup snapshot

```bash
pcloudc backup-snapshot-create ~/important \
  --gpg-recipient you@domain
# Snapshot created: snap-2026-04-16T10-50-00-abcd1234
# Encrypted to recipient: you@domain
# Uploaded 1,247 files (18.7 GiB) -> /Backups/.../snap-...
```

What this does:

1. Walks the local tree, computing content-addressed chunks.
2. Encrypts the archive to your GPG public key (`--gpg-recipient`).
3. Pushes the archive to `/Backups/…` via the transfer queue.
4. Writes a snapshot manifest to the SQLite store.

Verify:

```bash
pcloudc backup-snapshot-verify snap-2026-04-16T10-50-00-abcd1234
# manifest: ok (1,247 entries)
# chunks:   ok (all referenced chunks present server-side)
# signature: ok (signed-by you@domain)
```

Restore:

```bash
pcloudc backup-snapshot-restore snap-2026-04-16T10-50-00-abcd1234 \
  --gpg-recipient you@domain \
  --yes
```

Prune old snapshots:

```bash
pcloudc backup-snapshot-prune --retention-days 30 --yes
```

Compound-token equivalents also work (`backup snapshot-create …`,
`backup snapshot-verify …`) — they rewrite to the canonical
kebab tokens above.

> **Expert tip.** The snapshot is only as safe as your GPG key
> custody. Keep the private key off the backup host itself —
> otherwise a compromised host can both *write* new snapshots and
> *read* old ones. A typical layout: backup host holds the public
> key; a separate air-gapped recovery host holds the private key.

## 8. File-history preview (`log` / `diff` / `restore`)

```bash
pcloudc log "/Docs/readme.pdf"
# Error: Unavailable. Revision history is CLI-only scaffolding
#        until bd-1du.10 clears the public-API gate.

pcloudc diff "/Docs/readme.pdf" 3
pcloudc restore "/Docs/readme.pdf" 3
```

These commands parse and dispatch correctly, but the daemon returns
`Unavailable`. This is **by design** — the CLI surface is in place so
wrappers can be written now; the daemon side is part of the final
parity proof. Don't build a workflow on them yet.

## 9. Doctor — full probe sweep

```bash
pcloudc doctor
# Healthy baseline

pcloudc doctor --strict
# Same probes; any WARN becomes FAIL. Use in CI and image-baking.

pcloudc doctor --json | jq '.checks[] | select(.status != "ok")'
```

Probes (non-exhaustive):

| Probe | Meaning |
|---|---|
| `config` | `~/.config/pcloud-rs/config.toml` exists, is `0600`, parent is `0700`. |
| `runtime` | `~/.local/state/pcloud-rs/` exists, is `0700`, owned by you. |
| `socket` | IPC socket `0600`, peer-UID matches daemon's UID. |
| `tls` | Production config refuses plaintext downgrade. |
| `fuse` | A supported FUSE provider is installed (for mount). |
| `store` | SQLite store opens and passes `PRAGMA integrity_check`. |
| `vault` | If opted in, `vault.toml` is `0600`, ownership matches. |
| `network` | DNS resolves, TLS handshake to API succeeds. |
| `sync` | Every registered pairing's local path exists and is reachable. |

`--strict` promotes every WARN (unknown CA, weak parent dir, mismatched
SELinux label, unsigned daemon binary) to a FAIL.

> **Expert tip.** In your image-baking pipeline, gate the build on
> `pcloudc doctor --strict --json | jq -e '.overall == "ok"'`.
> That one line catches 90 % of "it worked on my laptop" regressions
> before they ship.

## FAQ

### <a id="faq-delete"></a>If I delete a file locally, does it delete on the remote?

**Yes, by default.** Bidirectional sync (`--direction both`) treats
local deletes as intent to delete on the remote. On the first delete
in a session, the CLI prompts:

```
You deleted 3 files under /home/you/work. Propagate to /Work? [y/N]
```

Options:

- `--confirm-deletes never` — always prompt.
- `--confirm-deletes always` — never prompt (not recommended
  interactively).
- `--direction up` — local creates mirror to remote but deletes do
  **not** propagate. This is the safe default for backup-style
  workflows.

Remote deletes land in the pCloud trash (recoverable for 30 days on
paid plans, 15 days on free). Recover:
`pcloudc trash list` / `pcloudc trash restore <fileid>`.

### What happens on a conflict?

If the same path changes on both sides between scans, the resolver
follows `[sync.conflict]`:

- `rename-loser` (default) — keep both; losing side renamed
  `name (conflict 2026-04-16 10:30:01).ext`.
- `prefer-local` — local wins; remote overwritten.
- `prefer-remote` — remote wins; local overwritten.
- `stop` — pairing paused, requires operator action.

Full details (tie-break on content hash, not wall clock) in the
[operations runbook](../operations/runbook.md).

### Does sync need the mount?

No. Classic sync is fully independent from the mount surface. The
mount is a separate subsystem and — on non-Linux platforms today —
is pre-alpha.

### Can I sync into a Crypto Folder?

Yes. Unlock first (`pcloudc unlock-crypto`), then
`pcloudc sync-add --crypto <LOCAL> <CRYPTO_REMOTE>`. The daemon
refuses a `--crypto` add if the remote is *not* inside a crypto
folder, and refuses a non-`--crypto` add pointing *into* a crypto
folder. That asymmetry prevents accidental plaintext uploads into
an encrypted namespace. Crypto folders are end-to-end encrypted;
losing the crypto password means losing the data.

### How do I pause sync temporarily?

```bash
pcloudc pause /home/you/work     # pause one pairing
pcloudc pause --all              # pause everything
pcloudc resume /home/you/work
```

Paused pairings stay registered; queues drain naturally on resume.

### How do I force a re-scan now?

```bash
pcloudc sync-localscan
```

Mirrors the C client's `psync_run_localscan` (aliases: `localscan`,
`run-localscan`, or `sync localscan`).

### Where does state live?

| Slot | Path (Linux) |
|---|---|
| Config | `~/.config/pcloud-rs/config.toml` (`0600`) |
| SQLite store | `~/.local/state/pcloud-rs/store.sqlite3` |
| Staging area | `~/.local/state/pcloud-rs/staging/` |
| Auth vault (opt-in) | `~/.local/state/pcloud-rs/vault.toml` |
| Daemon log | `~/.pcloud/state/daemon.log` |
| IPC socket | `~/.local/state/pcloud-rs/ipc.sock` |
| Page cache (mount) | `~/.local/state/pcloud-rs/page-cache/` |

Windows: `%APPDATA%\pcloud-rs\` and `%LOCALAPPDATA%\pcloud-rs\`.
macOS: `~/Library/Application Support/pcloud-rs/`.

## Troubleshooting — top five

1. **`Refusing to add: nested under an existing sync root`** — pick a
   non-overlapping path. Run `pcloudc sync-list` to see what you
   have.
2. **`Refusing to add: remote folder does not exist`** — either pass
   `--create-remote`, or run `pcloudc folder-create /Work` first.
3. **`mount: /mnt/pcloud Operation not permitted`** — missing FUSE
   provider, or your user isn't in the `fuse` group. On Debian:
   `sudo apt install fuse3 && sudo usermod -aG fuse $USER`, re-login.
4. **`publink create failed: quota exceeded`** — free accounts cap
   the number of active links. Delete an unused link
   (`pcloudc delete-link <id>`) or upgrade.
5. **`snapshot create failed: gpg: no public key for you@domain`** —
   import your public key first: `gpg --import you.pub`, verify with
   `gpg --list-keys you@domain`.

## Next steps

- [Operations handbook](../operations/runbook.md) — day-two: quota,
  bandwidth, conflicts, crypto lifecycle.
- [CLI reference](../reference/cli.md) — every subcommand and flag.
- [IPC protocol](../reference/ipc-protocol.md) — event schema,
  request/response enum, framing.
- [Security model](../security/model.md) — threat model, trust
  boundaries, what the daemon protects and what it does not.
- [STATUS](https://github.com/pcloudcom/pcloud-rs/blob/main/STATUS.md)
  — canonical parity counts. Mount and the `log`/`diff`/`restore`
  family are the honest open items.
