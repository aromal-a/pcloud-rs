# Exit Codes

`pcloud-rs` / `pcloudc` use a stable, documented set of exit codes. The
mapping is defined in `crates/pcloud-cli/src/exit_code.rs` and surfaced in
`pcloud-rs --help` via the `EXIT_CODE_HELP` constant.

> **Stable ABI.** These numeric values are part of the public CLI contract.
> Scripts, CI pipelines, systemd unit `RestartPreventExitStatus=`, Ansible
> `failed_when`, and orchestration tooling may rely on them. The values
> will not be renumbered. New categories will be added at the end. A unit
> test pins the numeric values so any accidental change is caught in
> review.

## Code Table

| Code | Variant | Meaning | Retryable? |
| --- | --- | --- | --- |
| `0` | `Ok` | Command completed successfully. | — |
| `1` | `GenericError` | Unclassified runtime failure. See stderr / JSON envelope for detail. | Maybe (inspect `error.code`). |
| `2` | `Usage` | Invalid CLI argument or missing subcommand parameter. | No — fix the invocation. |
| `3` | `Auth` | Authentication or authorization failure. Covers `AuthRequired` and `AuthFailed`. | After re-auth. |
| `4` | `Network` | Network / IPC transport failure. Daemon unreachable, TLS error, connection reset, peer-uid mismatch. | Yes, with backoff. |
| `5` | `CryptoLocked` | Crypto vault is locked or required key material is unavailable. | After `crypto start`. |
| `6` | `Unavailable` | Daemon or requested feature is disabled / not built in. | No, unless feature is enabled. |
| `7` | `Conflict` | Conflicting state: duplicate sync root, nested root, already-mounted target, already-running daemon when `serve` was requested. | Often not; re-check state. |
| `8` | `Internal` | Daemon reported an internal error. Treat as a bug. | Yes, once. |

## `ExitCode` Enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Ok           = 0,
    GenericError = 1,
    Usage        = 2,
    Auth         = 3,
    Network      = 4,
    CryptoLocked = 5,
    Unavailable  = 6,
    Conflict     = 7,
    Internal     = 8,
}
```

## `ResponseStatus` -> `ExitCode`

Performed by `ExitCode::from_response_status`:

| `ResponseStatus` | `ExitCode` |
| --- | --- |
| `Ok` | `Ok` (0) |
| `Usage` | `Usage` (2) |
| `AuthRequired` | `Auth` (3) |
| `AuthFailed` | `Auth` (3) |
| `NetworkError` | `Network` (4) |
| `CryptoLocked` | `CryptoLocked` (5) |
| `Unavailable` | `Unavailable` (6) |
| `Conflict` | `Conflict` (7) |
| `InternalError` | `Internal` (8) |
| `GenericError` | `GenericError` (1) |

When the CLI cannot reach the daemon at all (socket missing, peer-uid
rejection, framed-read failure) it returns `Network` (4) without
contacting the daemon.

When the CLI fails before IPC (argument parse error, TOML upsert failure,
config validation error surfaced via `doctor`) it returns `Usage` (2) or
`Unavailable` (6) as appropriate.

## Retryability Guidance

- **`Ok` (0)** — no action.
- **`GenericError` (1)** — inspect `error.code` in the `--json` envelope.
  Do not retry indefinitely.
- **`Usage` (2)** — never retry without changing the argv.
- **`Auth` (3)** — prompt for fresh credentials; re-submit TFA or
  recovery code; or re-run `login`.
- **`Network` (4)** — exponential backoff. Respect
  `resilience.retry_base_ms` / `resilience.retry_max_ms`. The daemon
  itself applies circuit-breaker discipline when
  `resilience.circuit_breaker_enabled` is `true`.
- **`CryptoLocked` (5)** — run `crypto start`, then retry the operation
  once.
- **`Unavailable` (6)** — confirm the daemon is running (`pcloud-rs start`)
  and that the relevant feature flag is set. Do not busy-loop.
- **`Conflict` (7)** — usually indicates state was already changed by
  another actor; re-check with `status` / `sync list` before retrying.
- **`Internal` (8)** — file an issue. A single retry is acceptable; more
  than one is not.

## JSON Envelope Correlation

In `--json` mode the exit code is mirrored inside the envelope:

```json
{
  "command": "sync add",
  "status": "conflict",
  "message": "local root /home/me/work is nested under existing root /home/me",
  "exit_code": 7,
  "error": { "code": "sync.nested_root", "detail": "..." }
}
```

The `exit_code` field always matches the process exit status. Tools that
cannot observe the exit status (e.g. piped pipelines where the child is
not the wait-ed process) can rely on the envelope field.

## Systemd / CI Examples

Systemd unit that refuses to restart on config errors or usage bugs:

```ini
[Service]
ExecStart=/usr/bin/pcloudd serve
Restart=on-failure
RestartPreventExitStatus=2 6
```

CI step that treats network flakes as retryable and auth failures as
fatal:

```bash
set +e
pcloud-rs --json status
rc=$?
case "$rc" in
  0)  ;;
  3)  echo "auth failure, failing build"; exit 1 ;;
  4)  echo "network flake, retrying"; exit 75 ;;   # EX_TEMPFAIL
  *)  echo "unexpected rc=$rc"; exit 1 ;;
esac
```

## See Also

- [CLI Reference](./cli.md)
- [IPC Protocol](./ipc-protocol.md) for `ResponseStatus`.
- `crates/pcloud-cli/src/exit_code.rs` — source of truth.
