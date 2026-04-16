# ADR 0002: IPC Socket Framing

- Status: Accepted
- Date: 2026-04-15

## Context

The Rust daemon exposes a local control surface consumed by the CLI, the
SDK's out-of-process client, and by integration tests. The original C
client never formalised this — it used a mix of ad-hoc text commands and
debug endpoints — so we were free to choose a framing from scratch.

Reviewer 18 (REVIEW_FULL_02.md) raised two specific concerns:

1. Line-oriented text protocols are fragile under partial reads and mixed
   binary payloads (token blobs, crypto ciphertext, upload descriptors).
2. HTTP-over-Unix-sockets would drag in a full HTTP stack (keep-alive,
   chunked encoding, header parsing) for a purely local control channel
   where none of HTTP's features (proxies, caching, content negotiation)
   apply.

Constraints:

- Must be usable from Rust on both sides with zero extra runtime.
- Must tolerate arbitrary binary payloads including zero bytes.
- Must bound memory — a malformed frame must not let a peer request a
  gigabyte allocation.
- Must be testable with a minimal mock transport.

## Decision

IPC uses **binary length-prefixed frames** on a Unix domain socket
(`AF_UNIX`, `SOCK_STREAM`). Each frame is:

```
u32 length (big-endian, payload bytes, excluding the length prefix)
bytes payload (serialised Request or Response)
```

Payload encoding is `bincode` of the shared `Request` / `Response` enums
defined in `pcloud-proto`. A hard cap on `length` rejects oversize
frames before any allocation.

## Consequences

Good:

- Zero ambiguity on frame boundaries; no streaming parser state.
- Binary-safe by construction; no escaping rules.
- Mock transports in tests are a one-file affair — just a `Vec<u8>` and a
  length-reader helper.
- The on-wire representation matches the in-memory one, so fuzzing the
  decoder directly exercises the real surface.

Bad:

- Not human-readable; debugging requires a decoder. Mitigated by the
  `pcloud-cli` `--trace` mode and by unit tests that round-trip every
  variant.
- Version evolution requires either additive enum variants or an
  explicit version byte. We rely on `bincode`'s well-defined behaviour
  for added variants and reserve the right to introduce a version byte
  if we ever need a breaking change.
- Not directly callable from `curl`. Acceptable: this is a local control
  channel, not a public API.

Security:

- Socket lives in an `0700` per-user runtime directory with `0600` on the
  socket file itself (see ADR 0005).
- Length cap is enforced before allocation.
- Peer credentials are checked via `SO_PEERCRED` on accept; foreign-UID
  peers are rejected.

## Alternatives Considered

- **JSON over HTTP (Unix socket)**: rejected — drags in an HTTP stack for
  no benefit, text framing is lossy for binary payloads, and every
  endpoint becomes a stringly-typed URL rather than a typed enum.
- **Newline-delimited JSON**: rejected — embedded newlines in payloads
  force escaping, and partial-read handling is non-trivial.
- **gRPC / Cap'n Proto**: rejected — schema/tooling overhead for a single
  binary-to-binary local channel is disproportionate.
- **Raw bincode without a length prefix**: rejected — `bincode` can
  recover from well-formed streams but offers no defence against
  truncated / mixed frames; explicit framing is cheap and removes an
  entire class of bugs.
