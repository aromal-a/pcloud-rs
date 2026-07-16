# pcloud-proto

Typed pCloud protocol clients for pcloud-rs: auth, transfer, shares, public
links, backups — all over TLS (`rustls` with the `ring` provider).

## What this crate does

- Implements the JSON-over-HTTPS request/response surface for every pCloud
  endpoint used by the workspace.
- Owns the TLS client stack. Production config rejects plaintext fallback.
- Integrates with `pcloud-resilience` for retries, timeouts, and the circuit
  breaker.

## Public API entry points

- `auth_api`, `transfer_api`, `shares_api`, `public_links_api`,
  `backup_api`, `account_api` modules.
- `ProtoClient::new`, dispatch helpers.

## Usage

The crate is usually consumed through daemon backends and the internal
`pcloud-embedded-sdk`. The public `pcloud-sdk` talks only to daemon IPC.

## Features

None. TLS is always on in production.

## Security posture

- `rustls` + `webpki-roots` with `ring` as the crypto provider.
- No insecure endpoint override; transport changes must be explicit.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
