# Configuration Reference

> Authoritative source: `crates/pcloud-config/src/**/*.rs`. This page is
> regenerated from the Rust types — anything it says about a key, default,
> type, or validation rule must match the code. If the two drift, the code
> wins and this page is wrong; open an issue.

## Who this page is for

- **Beginner operators**: start with [File format](#file-format),
  [Location & discovery](#location--discovery), and
  [Environment overrides](#environment-overrides). Every other section is
  reference-grade: skim headings, read examples, skip the rest.
- **Experienced operators / SREs**: focus on
  [Precedence](#precedence-how-a-key-resolves),
  [Validation & refusal rules](#validation--refusal-rules), and the
  per-section *Tuning* boxes.
- **FAANG-grade readers (security / platform)**: read
  [On-disk envelope & migrations](#on-disk-envelope--migrations),
  [Schema validator](#schema-validator), and the **Security** bullet on
  every key. Non-enforced settings are explicitly called out.

## File format

**JSON, not TOML.** Earlier revisions of this document described a
`config.toml` layout; that was aspirational. The actual on-disk envelope
is a JSON document with a fixed shape defined in
`crates/pcloud-config/src/schema.rs`:

```json
{
  "version": 2,
  "profile": {
    "environment": "Production",
    "paths": { "...": "..." },
    "api": { "...": "..." },
    "extensions": { "...": "..." },
    "runtime": { "...": "..." },
    "features": { "...": "..." },
    "limits": { "...": "..." },
    "mount": { "...": "..." },
    "observability": { "...": "..." },
    "resilience": { "...": "..." },
    "data_residency": { "...": "..." }
  }
}
```

Every object sets `additionalProperties: false`: unknown keys at any
level fail the load with a JSON-pointer-precise error (see
[Schema validator](#schema-validator)).

> **No `[crypto]`, `[crypto.kms]`, or `[network]` section exists today.**
> Previous drafts referenced them as future work; they are not read by
> any code path in the current workspace and are rejected by the loader
> as unknown properties. Crypto is controlled via `features.crypto_enabled`
> plus runtime unlock commands, not a static block.

## Location & discovery

The loader does **not** implement automatic file discovery — the daemon
and CLI take an explicit `--config <path>` argument. The canonical
candidate paths returned by
`pcloud_config::loader::default_candidate_paths(home)` are:

| Platform | Candidate paths (in order) |
|---|---|
| Linux / *BSD | `$HOME/.config/pcloud/config.json`, then `$HOME/.pcloud/config.json` |
| macOS | same (XDG fallback is respected by `PcloudDirs::discover`) |
| Windows | `%APPDATA%\pcloud\pcloud-rs\config\config.json` |

The managed directory layout itself is resolved by
`PcloudDirs::discover()` in `crates/pcloud-config/src/paths.rs`:

| Platform | `config` | `state` | `cache` | `runtime` |
|---|---|---|---|---|
| Linux / *BSD | `$XDG_CONFIG_HOME/pcloud/pcloud-rs` | `$XDG_DATA_HOME/pcloud/pcloud-rs` | `$XDG_CACHE_HOME/pcloud/pcloud-rs` | `$XDG_RUNTIME_DIR/pcloud/pcloud-rs` (falls back to `<cache>/pcloud-rs-runtime`) |
| macOS | `~/Library/Application Support/com.pcloud.pcloud-rs` | same as config | `~/Library/Caches/com.pcloud.pcloud-rs` | `<cache>/pcloud-rs-runtime` |
| Windows | `%APPDATA%\pcloud\pcloud-rs\config` | `%APPDATA%\pcloud\pcloud-rs\data` | `%LOCALAPPDATA%\pcloud\pcloud-rs\cache` | `<cache>\pcloud-rs-runtime` |

`PCLOUD_ROOT=<abs-path>` re-roots **all four managed directories plus
the plugin dir** under `<root>/{config,state,runtime,cache,plugins}`.
Intended for multi-instance and tests.

**Legacy migration.** `~/.pcloud/` (pre-XDG layout) is **never** read
unless you set `PCLOUD_MIGRATE_LEGACY_PATHS=1` and explicitly call
`PcloudDirs::migrate_from_legacy_if_needed()`. Migration is *copy, not
move*, and skips destinations that already contain data.

## Validation & refusal rules

The loader (`crates/pcloud-config/src/loader.rs`) enforces in this
order:

1. **File permission check** (`check_permissions`). In `Production`,
   any file with `mode & 0o077 != 0` is refused with
   `ConfigError::InsecureConfigPermissions`. In `Development` / `Test`
   the same condition produces a `LoadedProfile::warnings` entry. The
   `--insecure-config` CLI flag downgrades Production to a warning,
   intended for dev only.
2. **JSON parse** (`serde_json::from_str`). Malformed JSON →
   `ConfigError::InvalidJson`.
3. **Version migration** (`migrate::migrate_to_current`). Envelopes at
   v0 (bare profile, no envelope) and v1 (envelope without
   `observability`) are promoted in-memory; v>`CURRENT_VERSION` (2) is
   refused with `MigrationError::TooNew`. Migration is **forward-only
   and in-memory** — the on-disk file is unchanged unless the caller
   rewrites it.
4. **Schema validation** (`schema::validate_document`). Hand-rolled
   draft-07 subset: `type`, `required`, `additionalProperties: false`,
   `enum`, numeric `minimum`/`maximum`, `properties`, homogeneous
   `items`. Every violation reports JSON pointer + line + column in one
   pass.
5. **Typed deserialization** (`serde_json::from_value` into
   `ConfigProfile`).
6. **Semantic validation** (`ConfigProfile::validate`):
   - `paths`: all four directories must be absolute
     (`ConfigError::PathMustBeAbsolute`).
   - `api`: `Production` rejects `ApiMode::Plaintext`; non-empty host /
     server_name / non-zero port & timeouts required in plaintext/TLS
     modes.
   - `extensions`: capability flags or trusted keys require
     `plugins_enabled=true`; `plugin_dir` must be absolute.
   - `runtime`: no group or other permission bits on any managed
     directory mode.
   - `mount`: `allow_other=true` with `owner_only_by_default=true` is
     rejected.

Validation aborts on the first violation. There is no partial success.

> **`pcloudc doctor`** runs the same load path early so operators can
> surface errors without attaching to a live daemon. A file that passes
> `doctor` is guaranteed to pass `pcloudd start` (in the same
> environment).

## Precedence (how a key resolves)

From lowest to highest precedence:

1. **Struct defaults** — `ConfigProfile::secure_defaults(root, env)`
   is the in-memory baseline when no file is present.
2. **On-disk envelope** — `profile.<key>` after migration + schema
   validation + typed deserialization.
3. **Targeted `PCLOUD_*` env var** — applied by
   `env::apply_env_overrides` *after* deserialize, *before*
   semantic validation. See the full table in
   [Environment overrides](#environment-overrides).
4. **Coarse `PCLOUD_ROOT`** — re-roots every path. Applied *first*
   inside `apply_env_overrides`, so targeted env vars below still win
   for their field.
5. **`PCLOUD_ENV` snap** — flipping the environment also snaps
   `api.mode` to that environment's secure default **unless**
   `PCLOUD_API_MODE` is also set (explicit mode wins).
6. **CLI flags** — `--env`, `--config`, `--insecure-config`. The CLI
   constructs `LoadOptions` and hands the loader an already-chosen
   path; there is no per-field CLI override today.

After all six layers are applied, `ConfigProfile::validate()` runs a
final pass, so an override that produces an invalid profile (e.g.
`PCLOUD_ENV=production` combined with `PCLOUD_API_MODE=plaintext`)
fails the load with `ConfigError::InvalidApiEndpoint` rather than
quietly accepting.

## On-disk envelope & migrations

- `version` is a `u32 >= 0`. `CURRENT_VERSION = 2`.
- **v0 → v1**: wrap bare-profile into `{version, profile}`. No data
  change. A pre-envelope file that never had a `version` field is
  treated as v0.
- **v1 → v2**: add the `observability` block using
  `ObservabilityFlags::secure_defaults()` when missing. Existing
  overrides preserved.
- Rollback is not supported. A v2 envelope is not readable by an older
  build — restore from backup if you must revert.
- Migration runs in memory only. The on-disk file is not rewritten by
  the loader.

## Schema validator

Defined verbatim as `schema::CONFIG_SCHEMA_JSON` (draft-07-compatible).
The Rust-side checker in `schema.rs` mirrors the JSON, adding precise
pointer + line/column diagnostics:

```
at /profile/api/port (line 14, col 19): value 70000 above maximum 65535
at /profile/api     (line 11, col 5):  unexpected property 'hsot' (additionalProperties=false)
at /profile/paths   (line 6,  col 5):  missing required property 'cache_dir'
```

External tools (IDE JSON-schema hints, `check-jsonschema` in CI) can
consume `CONFIG_SCHEMA_JSON` as a standalone schema document.

## Profile sections

Every section below corresponds to a submodule under
`crates/pcloud-config/src/`. Required fields are enforced by the
schema; absent optional fields use the serde `Default` impl.

### `profile.environment`

- **Type**: string enum `"Development" | "Test" | "Production"`.
- **Required**: yes.
- **Default**: Production for release builds, Development for debug.
- **Purpose**: pins transport + permission posture. Production refuses
  plaintext API transport, group/world-readable config files, insecure
  directory modes, and zero-valued API timeouts. Dev/Test downgrade the
  file-permission check to a warning.
- **Interactions**: `PCLOUD_ENV` overrides; if `PCLOUD_API_MODE` is not
  set, a fresh `environment` also snaps `api.mode` to the secure
  default for that environment.
- **Example**: `"environment": "Production"`.

### `profile.paths`

All four fields are required; all must be absolute.

| Key | Type | Default (Linux) | Purpose | Security | Example |
|---|---|---|---|---|---|
| `config_dir` | absolute string | `$XDG_CONFIG_HOME/pcloud/pcloud-rs` | Persistent user config + auth-token vault | Must be `0700` in Production (via `runtime.config_dir_mode`) | `/home/alice/.config/pcloud/pcloud-rs` |
| `state_dir` | absolute string | `$XDG_DATA_HOME/pcloud/pcloud-rs` | SQLite store, audit log, sync DB | `0700` in Production; holds sync metadata + audit trail | `/home/alice/.local/share/pcloud/pcloud-rs` |
| `runtime_dir` | absolute string | `$XDG_RUNTIME_DIR/pcloud/pcloud-rs` (falls back to `<cache>/pcloud-rs-runtime`) | IPC socket (`pcloud.sock`), PID file | `0700` — loosening leaks the IPC socket to other local users | `/run/user/1000/pcloud/pcloud-rs` |
| `cache_dir` | absolute string | `$XDG_CACHE_HOME/pcloud/pcloud-rs` | Thumbnails, FUSE staging, transient blobs | `0700` — FUSE staging may briefly hold plaintext of encrypted content | `/home/alice/.cache/pcloud/pcloud-rs` |

**Tuning**: for multi-instance isolation or CI, prefer `PCLOUD_ROOT`
over hand-editing each field. That one env var re-roots everything
(including the plugin dir) under a single tree.

**Derived paths** (not persisted, computed by helpers):

- IPC socket path = `runtime_dir/pcloud.sock`
  (`ManagedPaths::ipc_socket_path`).
- Auth-token vault = `config_dir/auth_token`
  (`ManagedPaths::auth_token_vault_path`).

### `profile.api`

All six fields are required.

| Key | Type | Default | Purpose | Validation | Security |
|---|---|---|---|---|---|
| `mode` | `"Development" \| "Plaintext" \| "Tls"` | Tls in Prod, Development in Dev/Test | Transport mode | Production rejects `Plaintext`; `Development` skips host-level checks (mocks/fixtures) | TLS mandatory in production (`ApiEndpoint::validate`) |
| `host` | string | `"bineapi.pcloud.com"` | DNS/IP of API endpoint | Must be non-empty in plaintext/TLS | Wrong value silently routes traffic |
| `port` | u16 (0–65535) | `443` | TCP connect port | Must be non-zero in plaintext/TLS | Only 443/TLS is sane in Production |
| `server_name` | string | `"bineapi.pcloud.com"` | TLS SNI + certificate verification name | Required non-empty in `Tls` mode | Setting to attacker-controlled value disables MITM protection |
| `connect_timeout_ms` | u64 | `5000` | TCP `connect()` timeout | Must be non-zero in plaintext/TLS | DoS bound — too high pins a worker on a stalled handshake |
| `read_timeout_ms` | u64 | `15000` | Per-read wait for framed data | Must be non-zero in plaintext/TLS | Caps slowloris-class hangs |

**Interactions**: `PCLOUD_API_MODE`, `PCLOUD_API_HOST`,
`PCLOUD_API_PORT`, `PCLOUD_API_SERVER_NAME`,
`PCLOUD_API_CONNECT_TIMEOUT_MS`, `PCLOUD_API_READ_TIMEOUT_MS`.

**Runtime hint**: the daemon's handler for `set_api_server` calls
`ApiEndpoint::apply_api_server_hint("host"|"host:port")`, which
updates both `host` and `server_name`; a bare host leaves `port`
untouched.

**Tuning**: default 5s/15s is intentionally tight — raise only for
high-RTT scenarios and always pair with a higher breaker reset
(`resilience.breaker_reset_timeout_ms`). `Development` mode is the
only way to point at a mock server on `127.0.0.1:0`.

### `profile.extensions`

Plugin loader policy. Secure default: everything off. Capability
grants or trusted keys with `plugins_enabled = false` are rejected
at validate time.

| Key | Type | Default | Purpose / Security |
|---|---|---|---|
| `plugins_enabled` | bool | `false` | Master switch. Loader skips the plugin dir entirely when off. Override: `PCLOUD_PLUGINS_ENABLED`. |
| `plugin_dir` | absolute string | `<root>/plugins` | Directory holding trusted plugin binaries/manifests. Must be absolute. Loader does **not** re-check its mode — keep it under the `0700` `config_dir`. |
| `allow_network_capability` | bool | `false` | Grants `network` capability (outbound sockets). Override: `PCLOUD_PLUGIN_ALLOW_NETWORK`. |
| `allow_sync_control_capability` | bool | `false` | Grants sync-lifecycle commands. Override: `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL`. |
| `allow_crypto_capability` | bool | `false` | Grants crypto-folder primitives. **Most sensitive** — plugin runs in-process and can read keys. Override: `PCLOUD_PLUGIN_ALLOW_CRYPTO`. |
| `trusted_plugin_keys` | array of `[u8; 32]` | `[]` | Ed25519 public keys authorized to sign manifests. Empty = "dev mode" (warn only). Non-empty makes signatures mandatory. No env override. |

**Tuning**: never enable `allow_network_capability` or
`allow_crypto_capability` on an untrusted manifest. Populating
`trusted_plugin_keys` is the only production-grade plugin posture;
dev mode (empty list + `plugins_enabled=true`) logs a warning on every
load.

### `profile.runtime`

Unix modes applied to each managed directory. All four are required;
all must satisfy `mode & 0o077 == 0`.

| Key | Default | Rejection |
|---|---|---|
| `config_dir_mode` | `0o700` (decimal `448`) | any group/other bit → `ConfigError::InsecureMode` |
| `socket_dir_mode` | `0o700` | same |
| `state_dir_mode` | `0o700` | same |
| `cache_dir_mode` | `0o700` | same |

**Security**: these are the modes the daemon enforces via `chmod`
before using each directory. No env override — modes are intentionally
not tunable from the environment to prevent accidental relaxation.

### `profile.features`

| Key | Type | Default | Purpose / Security |
|---|---|---|---|
| `p2p_enabled` | bool | `false` | Reserved for future P2P transfers. No runtime code reads this flag today; keep `false`. |
| `crypto_enabled` | bool | `true` | Enables crypto-folder support (AES-256-GCM sectors, metadata filename encoding, temppass share flow). **Do not disable** to "just see files" — encrypted blobs simply won't decrypt. |
| `durable_auth_tokens_enabled` | bool | `false` | Opt-in to persist auth tokens to the `0600` vault at `config_dir/auth_token`. **Passwords are never persisted regardless.** Override: `PCLOUD_DURABLE_AUTH_TOKENS`. |
| `integrity_sweeper` | object | off | Background-scrub block. See below. |

#### `profile.features.integrity_sweeper`

Optional block (every field has a serde default so older envelopes
load cleanly). Tracked under `bd-1du.4.6.1`.

| Key | Type | Default | Purpose |
|---|---|---|---|
| `enabled` | bool | `false` | Master switch. While `false`, no worker is spawned and no I/O happens. |
| `schedule_cron` | `Option<string>` | `null` | Reserved. An invalid cron expression will refuse to start the worker once the scheduler lands; today only on-demand runs are supported. |
| `rate_files_per_minute` | u32 | `100` | Token-bucket budget. Tokens accrue at `rate/60` per second, capped at the per-minute value. `0` permanently disables work. |
| `pause_on_battery` | bool | `true` | Wired-but-inert flag — no battery facade exists yet. Default is fixed now so the safe posture cannot regress once detection lands. |
| `skip_list_path` | `Option<string>` | `null` | Path to a newline-delimited glob file. Invalid globs return `io::ErrorKind::InvalidData` with a line number — never silently dropped. |

**Honest status**: the daemon worker is offline-triggered only; the
scheduler loop is not wired yet. `pcloudc integrity run-once` is the
end-to-end path today.

### `profile.limits`

Safety caps, not performance knobs. All three fields required.

| Key | Default | Purpose |
|---|---|---|
| `max_concurrent_uploads` | `4` | Caps uplink saturation and per-client memory. `0` halts uploads. |
| `max_concurrent_downloads` | `4` | Same for downloads. |
| `max_parser_frame_bytes` | `8 * 1024 * 1024` (8 MiB) | Hard cap on a single wire-protocol frame. Checked **before** allocating, so a hostile server cannot coerce multi-GiB allocations via a forged length prefix. |

**Tuning**: raising concurrency past ~16 rarely helps; pCloud's server
side already rate-limits per-account. `max_parser_frame_bytes` should
stay at 8 MiB — that is the documented API ceiling.

### `profile.mount`

FUSE mount policy and cache tuning.

| Key | Default | Semantics |
|---|---|---|
| `allow_other` | `false` | Pass `allow_other` to FUSE. Requires `owner_only_by_default=false` or validate fails with `InvalidMountPolicy`. Also needs `user_allow_other` in `/etc/fuse.conf`. |
| `owner_only_by_default` | `true` | Enforce owner-uid check + FUSE `default_permissions`. Secure default. Incompatible with `allow_other=true`. |
| `cache_size_mb` | `256` | Maximum page-cache memory budget in MiB. Controls how much file content the FUSE adapter keeps in RAM. Overridden at runtime by `PCLOUD_CACHE_SIZE_GB` (env var takes precedence). |
| `page_cache_entries` | `4096` | Maximum number of metadata-cache entries. Each entry caches one `getattr`/`lookup`/`readdir` result. LRU eviction applies. |
| `metadata_ttl_secs` | `60` | Metadata-cache TTL in seconds. Controls how long `getattr`/`lookup`/`readdir` results are served from cache before re-querying the remote. `0` disables caching. |

Valid combinations for access policy:

| `allow_other` | `owner_only_by_default` | Meaning |
|---|---|---|
| `false` | `true` | Default — owner-only mount. |
| `false` | `false` | Owner-only relaxed, still no `allow_other`. |
| `true` | `false` | Multi-user mount (explicit opt-in). |
| `true` | `true` | **Rejected** (`InvalidMountPolicy`). |

Environment-variable overrides for cache tuning:

| Env var | Maps to |
|---|---|
| `PCLOUD_MOUNT_CACHE_SIZE_MB` | `mount.cache_size_mb` |
| `PCLOUD_MOUNT_PAGE_CACHE_ENTRIES` | `mount.page_cache_entries` |
| `PCLOUD_MOUNT_METADATA_TTL_SECS` | `mount.metadata_ttl_secs` |

### `profile.observability`

Opt-in telemetry. Applied via `#[serde(default)]` so v1 envelopes load
cleanly. No env overrides are wired today.

| Key | Default | Purpose |
|---|---|---|
| `structured_logs_enabled` | `true` | JSON-lines logs to stderr. Redaction for `SecretString`/`SecretBytes` is always on regardless. Disabling falls back to unstructured stderr, not zero output. |
| `tracing_enabled` | `false` | OTEL-compatible span emission. Attributes go through the `attr_redact` allow-list; no secrets ever cross the boundary. |
| `metrics_enabled` | `false` | Prometheus text exposition on the owner-only IPC socket. Does not open any TCP port. |
| `audit_export_enabled` | `true` | Export auth / crypto / admin events to the persistent audit store. Disabling removes the forensic trail. |

### `profile.resilience`

Client-side rate limit + circuit breaker + retry policy used by
`pcloud-proto::ResilientTransport`. Present via `#[serde(default)]`;
existing direct-dispatch transports ignore this block.

| Key | Default | Tuning guidance |
|---|---|---|
| `enabled` | `true` | Master switch. Disabling removes both the limiter and breaker; prefer tuning fields below over flipping this off. |
| `rate_limit_capacity` | `16` | Burst size per endpoint. Caps instantaneous rate. |
| `rate_limit_refill_per_sec` | `8.0` | Sustained rate after burst drains. Positive finite `f64`. |
| `breaker_failure_threshold` | `5` | Consecutive failures before opening. Lower = faster fail-fast. |
| `breaker_reset_timeout_ms` | `30000` | Time Open before admitting a probe. Too short → thrash, too long → transient looks permanent. |
| `retry_max_attempts` | `3` | Total attempts including first. `1` disables retries. |
| `retry_base_delay_ms` | `100` | Initial delay before backoff. |
| `retry_factor` | `2.0` | Exponential factor. `1.0` = constant delay; `<1.0` is nonsensical and rejected by the transport. |
| `retry_max_delay_ms` | `5000` | Single-retry delay cap. |
| `retry_jitter_seed` | `0x00C0_FFEE_F00D` | Deterministic equal-jitter seed. Keeps tests reproducible while spreading retry storms across clients sharing the seed. |

### `profile.rate_limit`

Per-category, per-session IPC admission limiter consumed by
`pcloud-daemon::dispatch::handle_request`. Built from the validated
profile once at bootstrap and checked **before** the backend is
invoked. Over-budget callers receive
`ResponseStatus::Conflict` with a message of the form
`"rate limit exceeded: <category>, retry after Ns"` and the backend is
not called — so a hostile or misbehaving client cannot drain daemon
work budgets on expensive operations.

The section is optional via `#[serde(default)]`; envelopes that omit
it inherit the secure defaults below.

| Key | Default | Tuning guidance |
|---|---|---|
| `enabled` | `true` | Master switch. When `false`, every bucket degrades to "always allow". |
| `cheap`   | `{capacity: 0, refill_per_sec: 0.0}` (disabled) | Bucket for status / userinfo / field selectors. Zero capacity means "no limit". |
| `medium`  | `{capacity: 30, refill_per_sec: 0.5}` (≈ 30/min) | Bucket for list-style endpoints and single-item reads. |
| `expensive` | `{capacity: 6, refill_per_sec: 0.1}` (≈ 6/min) | Bucket for snapshot create, integrity run-once, bulk public-link operations, tree-link create, crypto password change. |

Category assignment lives in `pcloud_daemon::rate_limit::categorize`
and follows a conservative allow-list: only named cheap/expensive
methods are elevated; everything else sits in `medium`. Setting a
bucket's `capacity` to `0` disables that category without removing the
block.

**Example — tighter posture on the `expensive` bucket**:

```json
"rate_limit": {
  "enabled": true,
  "expensive": { "capacity": 3, "refill_per_sec": 0.05 }
}
```

### `profile.data_residency`

Region allow-list enforced at `sync-root add`, `upload_create`, and
`set_api_server` (live in `pcloud-backends::residency`). Optional block
via `#[serde(default)]`. **Empty allow-list = allow every region**, so
adding this block to an existing deployment is a no-op until populated.

| Key | Type | Default | Notes |
|---|---|---|---|
| `allowed_regions` | `Vec<String>` | `[]` | Case-insensitive region tags (`"EU"`, `"US"`, ...). |
| `strict` | bool | `false` | `true` → violations return `ResponseStatus::PolicyViolation { kind: "data_residency" }`. `false` → warning audit event, operation proceeds. |

**EU-only example**:

```json
"data_residency": { "allowed_regions": ["EU"], "strict": true }
```

## Environment overrides

Applied by `env::apply_env_overrides` *after* deserialize and *before*
semantic validation. Unset or whitespace-only vars are ignored; any
set-but-malformed value aborts the load with
`ConfigError::InvalidEnvironmentValue`.

**Boolean grammar** (case-insensitive): `1`/`0`, `true`/`false`,
`yes`/`no`, `on`/`off`.

**Enum grammars** (case-insensitive):

- `PCLOUD_ENV`: `dev`/`development`, `test`, `prod`/`production`.
- `PCLOUD_API_MODE`: `dev`/`development`, `plain`/`plaintext`/`tcp`,
  `tls`/`ssl`.

Full mapping (everything else is not wired — previous drafts mentioned
`PCLOUD_CACHE_SIZE_GB`, `PCLOUD_LOG_LEVEL`, `PCLOUD_METRICS_BIND_ALL`,
`PCLOUD_CHAOS`, etc.; none of those are read by `env.rs` today, so do
not rely on them).

| Env var | Target field | Notes |
|---|---|---|
| `PCLOUD_ROOT` | `paths.*`, `extensions.plugin_dir` | Coarse override. Re-roots every managed path and the plugin dir under `<root>/{config,state,runtime,cache,plugins}`. Applied first; targeted vars below still win for their field. |
| `PCLOUD_ENV` | `environment` | Also snaps `api.mode` to the env's secure default when `PCLOUD_API_MODE` is not set. |
| `PCLOUD_API_MODE` | `api.mode` | Wins over the `PCLOUD_ENV` snap. Production still rejects `Plaintext` at validate time. |
| `PCLOUD_API_HOST` | `api.host` | |
| `PCLOUD_API_PORT` | `api.port` | Must parse as `u16`. |
| `PCLOUD_API_SERVER_NAME` | `api.server_name` | TLS SNI / cert verification name. |
| `PCLOUD_API_CONNECT_TIMEOUT_MS` | `api.connect_timeout_ms` | `u64`; `0` fails validation in plaintext/TLS. |
| `PCLOUD_API_READ_TIMEOUT_MS` | `api.read_timeout_ms` | `u64`; `0` fails validation in plaintext/TLS. |
| `PCLOUD_PLUGINS_ENABLED` | `extensions.plugins_enabled` | |
| `PCLOUD_PLUGIN_ALLOW_NETWORK` | `extensions.allow_network_capability` | Requires `plugins_enabled=true`. |
| `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL` | `extensions.allow_sync_control_capability` | Requires `plugins_enabled=true`. |
| `PCLOUD_PLUGIN_ALLOW_CRYPTO` | `extensions.allow_crypto_capability` | Requires `plugins_enabled=true`. |
| `PCLOUD_DURABLE_AUTH_TOKENS` | `features.durable_auth_tokens_enabled` | Gates the on-disk auth-token vault. |
| `PCLOUD_MIGRATE_LEGACY_PATHS` | (not a profile field) | When `=1`, enables opt-in copy of `~/.pcloud/` into the XDG layout on first-run migration. |

### Example: multi-instance under a test root

```bash
export PCLOUD_ROOT=/tmp/pcloud-test-42
export PCLOUD_ENV=test
export PCLOUD_API_MODE=development
pcloudd start
```

### Example: production with explicit API endpoint override

```bash
export PCLOUD_ENV=production
export PCLOUD_API_HOST=bineapi-eu.pcloud.com
export PCLOUD_API_SERVER_NAME=bineapi-eu.pcloud.com
# PCLOUD_API_MODE intentionally unset — PCLOUD_ENV=production snaps to Tls.
pcloudd start
```

## Reload semantics

The daemon loads the profile **once at start-up** and snapshots it into
the `RuntimeShell`. There is no atomic SIGHUP reload today: config
changes require `pcloudc stop` + `pcloudd start`. Live-reload is
tracked in a future wave and will preserve the validate-then-swap
invariant — if the new file fails validation, the old profile stays
active and an audit event is emitted.

## Troubleshooting

### `InsecureConfigPermissions`

```
config file '/home/alice/.config/pcloud/config.json' has insecure
permissions (mode 644): refusing to load. Pass --insecure-config to
override in development.
```

Fix: `chmod 600 /home/alice/.config/pcloud/config.json` and confirm
the parent directory is `0700` (`chmod 700 ~/.config/pcloud`). The
`--insecure-config` flag is a dev-only escape hatch; Production still
warns even when the flag is set.

### `SchemaViolations`

Every violation carries a JSON pointer and (when locatable in the raw
text) a line/column. Unknown properties are rejected with the
`additionalProperties=false` phrase — use that as your grep key when
bulk-fixing envelopes.

### `InvalidApiEndpoint`

- `"production environment requires tls api mode"` — flip `api.mode`
  to `"Tls"` or change `environment` to `"Development"`/`"Test"`.
- `"host must not be empty"` / `"server_name must not be empty in tls
  mode"` — self-explanatory.
- `"connect_timeout_ms must be non-zero"` — `0` is never valid in
  plaintext/TLS; it would mean "give up before starting".

## See also

- [CLI Reference](./cli.md) — operator surface.
- [IPC Protocol](./ipc-protocol.md) — the daemon's local wire format
  that the `runtime_dir` socket exposes.
- `crates/pcloud-config/src/schema.rs` — authoritative JSON schema.
- `crates/pcloud-config/src/env.rs` — authoritative env-var override
  table.
- `crates/pcloud-config/src/loader.rs` — permission check + migration
  + validation pipeline.
