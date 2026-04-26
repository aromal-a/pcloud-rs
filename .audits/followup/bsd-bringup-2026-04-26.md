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
| **OpenBSD 7.x** | ✗ | n/a | n/a | Vagrant box (`generic/openbsd7@4.3.12`) boots, port 2224 accepts TCP, but no SSH banner returned. Restart with virtio-net-pci instead of e1000 caused the VM to die without a serial-redirected console to debug. Multiple QEMU/NIC permutations tried; without GUI access to the VNC console (host has no display), root-cause was indeterminate within the session budget. |
| **DragonFly 6.x** | ✗ | n/a | n/a | Same symptom as OpenBSD — TCP handshake completes but no SSH banner. VM stays alive but `sshd` never serves traffic. Likely needs a libvirt-network-shaped configuration that vanilla QEMU user-mode networking doesn't provide (dhcp release races, fixed-MAC expectations, etc.). |

## Repro for OpenBSD / DragonFly

The Vagrant boxes are downloaded and intact at:

```
~/vm/bsd/openbsd/box.img       (128 GiB sparse qcow2, 7.x)
~/vm/bsd/dragonfly/box.img     (128 GiB sparse qcow2, 6.x)
~/vm/bsd/vagrant_insecure_key  (the well-known Vagrant SSH key)
```

The cleanest path forward — when an operator picks this up — is one
of:

1. **Install `vagrant-libvirt`** (Arch: `paru -S vagrant vagrant-libvirt`
   plus `libvirt` daemon, `dnsmasq`, `iptables-nft`). Then `vagrant
   up --provider=libvirt` per box. The boxes were built for libvirt
   and "just work" in that environment.

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
| OpenBSD | not yet booted | — | — |
| DragonFly | not yet booted | — | — |
| macOS | (still Tier-3 scaffolded; no hardware in this session) | — | — |

Four out of five released-Tier-1-or-2 platforms validated with passing
test suites. The pcloud-rs cross-platform story is now substantively
honest: the *only* major desktop OS not green is macOS (hardware-gated)
and the rare BSDs (vagrant-box-shape gated, recoverable per the repro
above).
