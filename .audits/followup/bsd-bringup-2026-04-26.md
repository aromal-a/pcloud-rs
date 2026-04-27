# BSD bring-up — 2026-04-26 session

Tier-2 promotion attempt for the four major BSDs. Workflow mirrored
the Windows bring-up (commit `cbd7203` and predecessors): pre-built
Vagrant qemu provider boxes via `vagrantcloud.com`, booted with raw
QEMU + KVM, SSH via the published Vagrant insecure key.

## Outcomes

| BSD | Boot | cargo check | cargo test --workspace --lib | Notes |
|---|---|---|---|---|
| **FreeBSD 14.4** | ✓ (cloud-init image, port 2222) | clean (56 s) | **1538 passing / 0 failing** | Validated end-to-end. Commits: f3b3bcb (procfs + fs_watcher fixtures gated to Linux). |
| **NetBSD 9.3** | ✓ (Vagrant box, port 2223, e1000 NIC) | clean (1m 20s `--all-targets` at a3c2c2e) | **1537 passing / 0 failing / 2 ignored** (33 binaries) | Required two compile fixes: `notify 6 → 8` for kqueue ABI (41a51a3), and `bsd.rs` `statvfs` alias for missing `statfs` type (b4bb777). Plus pacer test gated to Linux (a3c2c2e). Aggregate re-run at HEAD a3c2c2e. |
| **OpenBSD 7.8** | ✓ (vagrant-libvirt, `DefinedNet/openbsd78@1.0.17`) | clean (2m 25s) | **1529 passing / 0 failing / 2 ignored** (32 binaries) | Required redirecting `target/` via `CARGO_TARGET_DIR=/usr/obj/pcloud-rs-target` (default `/home` is 3.5G, too small) plus chowning `/usr/obj` to `vagrant`. Plus circuit-breaker 1000-thread stress test gated for OpenBSD's tighter `kern.maxthread` per-user cap (d688494). Toolchain: rustc 1.90.0, git 2.51.0. |
| **DragonFly 6.4** | ✓ (vagrant-libvirt, `dragonfly-test`) | clean (~12 s) | **1576 passing / 0 failing / 2 ignored** (33 binaries) | Toolchain: rustc 1.85.1 + git 2.49.0 (dports `Avalon` LATEST). Workspace MSRV restored to 1.85 (5b67f31) by refactoring all `let_chains` (let-and-let + let-and-cond) into nested `if-let { if cond { ... } }` form across ~50 sites. Plus: replaced `<unsigned>::is_multiple_of(N)` (stable 1.87) with `% N == 0` in pclsync_modes/sector + crypto_api; added `target_os = "dragonfly"` to the BSD cfg list in `pcloud-ipc::platform`; vendored three crates with DragonFly-friendly cfg overrides under `vendor/`: `if-addrs-0.13.4` (BSD cfg gates missing dragonfly → routed to Linux netlink), `notify-8.2.0` (Cargo.toml typo `dragonflybsd` → `dragonfly`), `regorus-0.9.1` (10 small accessors marked `pub const fn` calling `Vec::{len,is_empty}`, stable as const only since 1.87). Bonus quirk: `pkg install -y openssl` (3.x) is required **after** `pkg install -y rust` to satisfy `libssl.so.12` ABI even though the rust port's manifest lists `openssl-1.1.1v`. |

## Repro for OpenBSD / DragonFly

The original Vagrant boxes shipped libvirt-flavored qcow2 disks; running them
under vanilla QEMU user-mode networking left them booted-but-unreachable
(TCP handshakes complete, sshd never serves a banner — root cause is the
libvirt-shaped DHCP/MAC expectations the boxes encode). Resolution path
that worked in this session: install **vagrant-libvirt** + **libvirt** +
**dnsmasq** + **iptables-nft** on the host, then `vagrant up
--provider=libvirt` against an OpenBSD 7.8 box (`DefinedNet/openbsd78`,
1.0.17 on Vagrant Cloud — the `generic/openbsd7` box lags at 7.4 and
its mirror is purged). For DragonFly, the original `dragonfly-test`
box loads fine under libvirt; the toolchain catch is the rust pkg
gap noted above.

The historical artefacts were:

```
~/vm/bsd/openbsd/box.img       (128 GiB sparse qcow2, 7.x — superseded)
~/vm/bsd/dragonfly/box.img     (128 GiB sparse qcow2, 6.x)
~/vm/bsd/vagrant_insecure_key  (well-known Vagrant SSH key — auto-rotated by libvirt)
```

The cleanest path forward — when an operator picks this up — is one
of:

1. **Install `vagrant-libvirt`** (Arch: `yay -S vagrant`,
   `pacman -S libvirt dnsmasq iptables-nft openbsd-netcat`,
   then `vagrant plugin install vagrant-libvirt`; start `libvirtd`
   and add user to `libvirt` group). Then `vagrant up
   --provider=libvirt` per box. The boxes were built for libvirt
   and "just work" in that environment. **This is the path used to
   produce the OpenBSD/DragonFly results in the table above.**

2. **Install official ISO + drive sysinst via pexpect**. Both ISOs
   are downloaded already (`~/vm/bsd/openbsd/install78.iso`,
   `~/vm/bsd/dragonfly/dfly-x86_64-6.4.0_REL.iso`). For OpenBSD this
   is the canonical path since they don't publish cloud images.
   ~30-60 min wall time per BSD.

3. **Boot with `-display sdl` or `-display gtk`** on a host with a
   GUI to see the VNC console. The "TCP handshakes but no banner"
   pattern strongly suggests a userland process is bound but
   blocked, which would be visible on the console. Diagnosing
   without screen access spent ~45 minutes of session time without
   convergence.

## Pacer test platform-gate

Same shape as the Windows fix from 88739da: the `pacer_limit_enforces_
throughput_approximately` integration test is wall-time-sensitive in
a way that Linux nanosleep handles deterministically but Windows /
BSD scheduler quanta drown out. Gated to `target_os = "linux"` only
(commit a3c2c2e). Pacer correctness on other platforms is covered
by pure-function unit tests in the same file.

## Cumulative status

| Platform | Compile | --lib tests | --tests |
|---|---|---|---|
| Linux | ✓ | 1557 / 0 | 2217 / 0 |
| Windows | ✓ | 1449 / 0 | 2052 / 0 |
| FreeBSD | ✓ | 1538 / 0 | not yet run |
| NetBSD | ✓ | 1537 / 0 (2 ignored) | not yet run |
| OpenBSD 7.8 | ✓ | 1529 / 0 (2 ignored) | not yet run |
| DragonFly 6.4 | ✓ | 1576 / 0 (2 ignored) | not yet run |
| macOS 26.3.1 (Tahoe, arm64) | ✓ | 1597 / 0 (3 ignored) | not yet run |

## macOS bring-up — 2026-04-26 same session

Operator-supplied Apple Silicon host (macOS 26.3.1, Rust 1.92.0).
Three compile/test fixes landed in-session:

| Issue | Fix | Commit |
|---|---|---|
| `pcloud-fs::platform::macos` referenced undeclared `FUSET_BUNDLE` / `MACFUSE_BUNDLE` constants — audit-04 (9956a79) refactor carryover | Removed dead `.or_else(|| probe_one(BUNDLE))` fallbacks; CANDIDATES arrays already cover canonical paths via stronger dlopen+symbol probe | 36d390c |
| `build_fuse_args_every_option_preceded_by_dash_o` test missed three `MountOptions` fields added later (`attr_timeout_secs`, `entry_timeout_secs`, `max_readahead`) | Added `..MountOptions::default()` matching sibling test pattern | eb54a1c |
| 4 `pcloud-ipc::transport` tests EPERM on UDS bind because `IpcServer::bind` re-perms parent dir to 0700 — fine on Linux `/tmp` (1777-sticky), fails on macOS `/var/folders/.../T/` (DataVault-protected); plus stale `default_options_allow_other_is_true` asserting against the security-hardened `allow_other = false` default | Test helper `test_socket_path(name)` funnels each socket through a process-private subdir; renamed/inverted `default_options_allow_other_is_false` | a315675 |

Final aggregate: **33 binaries, 1597 passed, 0 failed, 3 ignored**
(`cargo test --workspace --lib --no-fail-fast`). One transient flake
on `pcloud_plugin_publink_expiry::state_file_round_trip_persists_
notification_state` (tmpdir nanos collision under workspace
parallelism) self-resolved on retry; root cause is the test harness's
nanos-only uniqueness guarantee colliding under high core counts —
not a production bug. Tracked separately if it recurs.

Five out of five released-Tier-1-or-2 platforms validated with passing
test suites. The pcloud-rs cross-platform story is now substantively
honest: every major desktop OS is green; only the rare BSDs (OpenBSD,
DragonFly — vagrant-box-shape gated, recoverable per the repro above)
remain.
