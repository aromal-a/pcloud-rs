# `pcloud-chaos`

**Maturity:** Verification support

**Version:** `0.1.0`

**Directory:** `crates/pcloud-chaos`

**Manifest:** [`crates/pcloud-chaos/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/Cargo.toml)

Chaos test harness for pcloud-rs (P1.6). Scripted fault-injection scenarios with predicted outcomes.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_chaos` | lib | [`crates/pcloud-chaos/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/src/lib.rs) |
| `blackhole_trips_breaker` | test | [`crates/pcloud-chaos/tests/blackhole_trips_breaker.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/blackhole_trips_breaker.rs) |
| `clock_jump_ttl` | test | [`crates/pcloud-chaos/tests/clock_jump_ttl.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/clock_jump_ttl.rs) |
| `disk_full_journal` | test | [`crates/pcloud-chaos/tests/disk_full_journal.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/disk_full_journal.rs) |
| `sigkill_mid_flush` | test | [`crates/pcloud-chaos/tests/sigkill_mid_flush.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/sigkill_mid_flush.rs) |
| `slowloris_timeout` | test | [`crates/pcloud-chaos/tests/slowloris_timeout.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/slowloris_timeout.rs) |

## Direct dependencies

`libc`, `pcloud-resilience`, `tempfile`, `thiserror`, `tokio`

## Cargo features

No declared package features.

## File inventory (8)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-chaos/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/Cargo.toml) | Cargo manifest | This crate intentionally has no library surface. It exists purely to host |
| [`crates/pcloud-chaos/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/README.md) | documentation | pcloud-chaos |
| [`crates/pcloud-chaos/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/src/lib.rs) | library root | pcloud-chaos |
| [`crates/pcloud-chaos/tests/blackhole_trips_breaker.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/blackhole_trips_breaker.rs) | test | Scenario 3: blackhole at connect → circuit breaker trips, retries back off. |
| [`crates/pcloud-chaos/tests/clock_jump_ttl.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/clock_jump_ttl.rs) | test | Scenario 4: 30 s clock jump forward → TTL-based caches re-fetch, don't crash. |
| [`crates/pcloud-chaos/tests/disk_full_journal.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/disk_full_journal.rs) | test | Scenario 2: disk-full on journal write → typed error, no panic. |
| [`crates/pcloud-chaos/tests/sigkill_mid_flush.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/sigkill_mid_flush.rs) | test | Scenario 1: SIGKILL mid-flush → journal replay completes without panic. |
| [`crates/pcloud-chaos/tests/slowloris_timeout.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/slowloris_timeout.rs) | test | Scenario 5: slowloris partial response → per-request timeout fires. |

## Rust declaration index (20 total; 4 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `chaos_enabled` | `pub` | fn | [`crates/pcloud-chaos/src/lib.rs:147`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/src/lib.rs#L147) | Returns `true` when the opt-in chaos env flag `PCLOUD_CHAOS` is set to exactly `"1"`. Scenarios that are expe… |
| `skip` | `pub` | fn | [`crates/pcloud-chaos/src/lib.rs:168`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/src/lib.rs#L168) | Prints a standard skip banner to stderr and returns `true` so the caller can immediately `return` from a test… |
| `chaos_blackhole_trips_breaker` | `private` | fn | [`crates/pcloud-chaos/tests/blackhole_trips_breaker.rs:28`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/blackhole_trips_breaker.rs#L28) | Read the source/rustdoc for the exact contract. |
| `TtlCache` | `private` | struct | [`crates/pcloud-chaos/tests/clock_jump_ttl.rs:25`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/clock_jump_ttl.rs#L25) | Minimal TTL cache used only for this chaos scenario. It mirrors the daemon's TTL-cache contract (`fetch` on m… |
| `new` | `private` | fn | [`crates/pcloud-chaos/tests/clock_jump_ttl.rs:32`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/clock_jump_ttl.rs#L32) | Read the source/rustdoc for the exact contract. |
| `get_or_fetch` | `private` | fn | [`crates/pcloud-chaos/tests/clock_jump_ttl.rs:40`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/clock_jump_ttl.rs#L40) | Read the source/rustdoc for the exact contract. |
| `chaos_clock_jump_invalidates_ttl` | `private` | fn | [`crates/pcloud-chaos/tests/clock_jump_ttl.rs:55`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/clock_jump_ttl.rs#L55) | Read the source/rustdoc for the exact contract. |
| `JournalError` | `private` | enum | [`crates/pcloud-chaos/tests/disk_full_journal.rs:20`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/disk_full_journal.rs#L20) | Read the source/rustdoc for the exact contract. |
| `chaos_disk_full_journal` | `private` | fn | [`crates/pcloud-chaos/tests/disk_full_journal.rs:29`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/disk_full_journal.rs#L29) | Read the source/rustdoc for the exact contract. |
| `unix_impl` | `private` | mod | [`crates/pcloud-chaos/tests/disk_full_journal.rs:46`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/disk_full_journal.rs#L46) | Read the source/rustdoc for the exact contract. |
| `set_fsize_limit` | `private` | fn | [`crates/pcloud-chaos/tests/disk_full_journal.rs:51`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/disk_full_journal.rs#L51) | Read the source/rustdoc for the exact contract. |
| `journal_append` | `private` | fn | [`crates/pcloud-chaos/tests/disk_full_journal.rs:66`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/disk_full_journal.rs#L66) | Read the source/rustdoc for the exact contract. |
| `run` | `pub` | fn | [`crates/pcloud-chaos/tests/disk_full_journal.rs:89`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/disk_full_journal.rs#L89) | Read the source/rustdoc for the exact contract. |
| `chaos_sigkill_mid_flush` | `private` | fn | [`crates/pcloud-chaos/tests/sigkill_mid_flush.rs:33`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/sigkill_mid_flush.rs#L33) | Read the source/rustdoc for the exact contract. |
| `unix_impl` | `private` | mod | [`crates/pcloud-chaos/tests/sigkill_mid_flush.rs:50`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/sigkill_mid_flush.rs#L50) | Read the source/rustdoc for the exact contract. |
| `append_frame` | `private` | fn | [`crates/pcloud-chaos/tests/sigkill_mid_flush.rs:60`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/sigkill_mid_flush.rs#L60) | Writes a single 32-byte record: \[u32 len\]\[u32 crc\]\[24 bytes payload\] fsync'd before returning. Matches the du… |
| `replay` | `private` | fn | [`crates/pcloud-chaos/tests/sigkill_mid_flush.rs:74`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/sigkill_mid_flush.rs#L74) | Replays, returning the number of successfully-parsed frames. Partial / torn trailing frames are dropped (not… |
| `run` | `pub` | fn | [`crates/pcloud-chaos/tests/sigkill_mid_flush.rs:104`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/sigkill_mid_flush.rs#L104) | Read the source/rustdoc for the exact contract. |
| `READ_CAP` | `private` | const | [`crates/pcloud-chaos/tests/slowloris_timeout.rs:26`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/slowloris_timeout.rs#L26) | Read the source/rustdoc for the exact contract. |
| `chaos_slowloris_timeout` | `private` | fn | [`crates/pcloud-chaos/tests/slowloris_timeout.rs:30`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-chaos/tests/slowloris_timeout.rs#L30) | Read the source/rustdoc for the exact contract. |

## Usage guidance

This package proves behavior and is not a shipped end-user runtime surface.
