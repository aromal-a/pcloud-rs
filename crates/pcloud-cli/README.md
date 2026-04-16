# pcloud-cli

Command-line client (`pcloudc`) for pcloud-rs that talks to the local daemon
(`pcloudd`) over secure owner-only UNIX-socket IPC.

## What this crate does

- Parses user commands with `clap`.
- Serializes requests through `pcloud-ipc` and renders responses.
- Never talks to the pCloud network directly — all traffic flows through the
  daemon, which keeps secrets and transport policy in one place.

## Public API entry points

- The `pcloudc` binary at `src/main.rs`.
- A small library surface for testing argument parsing and IPC round-trips.

## Usage

```text
pcloudc login --email alice@example.com
pcloudc sync add --local ~/pcloud --remote /
pcloudc status
```

## Features

None. Single default build.

## Security posture

- Passwords are read via `rpassword`, never echoed and never written to shell
  history.
- Socket path permissions are validated before any secret is sent.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
