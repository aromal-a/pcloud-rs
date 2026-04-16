# Property, Fuzz, and Stress Tests

This document describes the property-based tests, libFuzzer harnesses, and
stress workloads added for the pcloud-rs Rust rewrite's enterprise hot paths.

All artifacts are **test-only**. No production code is modified by these
harnesses. If a harness uncovers a bug, report it — do not fix it in the same
change.

## 1. Property tests (proptest)

Run as part of the standard workspace test suite:

```bash
cd .
cargo test --workspace
```

Files:

| Crate           | File                                                         | Covers                                                                                                                                         |
|-----------------|--------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------|
| `pcloud-ipc`    | `tests/proptest_methods_roundtrip.rs`                        | Every `Method` variant round-trips; random structural `Request` variants; random frames do not panic; random `Response` round-trips            |
| `pcloud-proto`  | `tests/proptest_response_and_frames.rs`                      | Binary request-encoder frame-length invariants; over-long param names rejected; random bytes never panic response parser; limits are enforced |
| `pcloud-secret` | `tests/proptest_zeroize_invariants.rs`                       | `SecretBytes` / `SecretString` round-trip, `Debug` redaction, constant-time eq matches structural eq, `zeroize()` empties exposed buffer       |
| `pcloud-daemon` | `tests/proptest_sync_and_resolver.rs`                        | Sync-root canonicalization classifier state transitions; `StaticPublicLinkPathResolver` state-transition invariants                            |

## 2. libFuzzer targets (cargo-fuzz)

cargo-fuzz is nightly-only. The `fuzz/` directories are deliberately excluded
from the workspace (`[workspace]` stub in each `fuzz/Cargo.toml`) so standard
`cargo check --workspace` / `cargo test --workspace` remain stable on stable
Rust.

Install once:

```bash
cargo install cargo-fuzz
rustup toolchain install nightly
```

### IPC frame fuzzer

```bash
cd crates/pcloud-ipc/fuzz
cargo +nightly fuzz run fuzz_ipc_frame
```

Feeds arbitrary bytes to `decode_request` and `decode_response`. Must never
panic, over-read, or allocate unbounded memory.

### Proto response-parser fuzzer

```bash
cd crates/pcloud-proto/fuzz
cargo +nightly fuzz run fuzz_response_parser
```

Feeds arbitrary bytes to `pcloud_proto::response::parse_response_frame`. Must
respect documented `ParseLimits` and never panic.

### Proto binary-request encoder fuzzer

```bash
cd crates/pcloud-proto/fuzz
cargo +nightly fuzz run fuzz_binary_request_roundtrip
```

Constructs random structured requests and exercises
`pcloud_proto::binary_api::encode_request`. Every successful encoding must
stay within `MAX_REQUEST_FRAME_LEN` and preserve declared command / parameter
counts.

## 3. Stress tests

Stress tests are gated with `#[ignore]` and a `stress` substring to keep the
default workspace run fast.

### Concurrent IPC clients

```bash
cargo test --release -p pcloud-ipc -- --ignored stress
```

Spawns **50 client threads × 500 sequential requests each** (25 000 requests)
against a dev-mode owner-only Unix-socket server, asserting:

- no panic / deadlock,
- every response is `ResponseStatus::Ok`,
- the server's open-fd drift over baseline stays within a 64-descriptor
  ceiling,
- the socket file is cleaned up on `BoundIpcServer::drop`.

## 4. Workspace validation

After adding or modifying any of the above, re-run:

```bash
cd .
cargo check --workspace
cargo test --workspace
```

The `fuzz/` subdirectories must NOT appear in workspace default-members and
must NOT be required for the workspace check / test to pass.
