# pcloud-p2p

Peer-to-peer LAN sync scaffolding for pcloud-rs.

## What this crate does

- Holds the experimental LAN-peer discovery and content-addressed exchange
  primitives that let same-account peers short-circuit uploads over the local
  network.
- This crate is scaffolding only and is **not** wired into any production
  runtime path yet.

## Public API entry points

- `PeerAnnouncement`, `PeerRegistry`.

## Features

None.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
