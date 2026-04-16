# pcloud-daemon-win

**PLATFORM:** Windows only. On Linux/macOS/FreeBSD this crate compiles to an
empty translation unit (`#![cfg(windows)]` guard) and produces no binary.

`pcloudd-svc.exe` is a thin Windows Service wrapper around the `pcloud-daemon`
runtime. It registers with the Service Control Manager (SCM), translates
`Stop` / `Shutdown` signals into a cooperative shutdown flag, and hosts the
daemon worker inside the SCM-managed process.

This crate depends on the real sync daemon, `pcloudd.exe` (built from
`crates/pcloud-daemon`). The service binary and the daemon binary are expected
to live side-by-side under `C:\Program Files\pcloud-rs\`.

## Install

Run an elevated `cmd.exe` or PowerShell:

```cmd
sc.exe create pcloudd binPath= "C:\Program Files\pcloud-rs\pcloudd-svc.exe" start= auto
sc.exe description pcloudd "pCloud sync daemon (pcloud-rs)"
```

Note the literal space after `binPath=` and `start=` — `sc.exe` requires it.

## Start / Stop

```cmd
sc.exe start   pcloudd
sc.exe stop    pcloudd
sc.exe query   pcloudd
```

Or via `services.msc` in the GUI.

## Uninstall

```cmd
sc.exe stop   pcloudd
sc.exe delete pcloudd
```

## Dependencies

* `pcloudd.exe` — the actual sync daemon. This wrapper is a shim; without
  `pcloud-daemon` linked in, it has nothing to host.
* `windows-service` crate (0.7) — SCM FFI bindings.

## Status

The cooperative shutdown path is a temporary shim: on SCM `Stop` we flip an
`Arc<AtomicBool>` and return. A proper `pcloud_daemon::serve_with_shutdown`
entry point is tracked under `bd-1du.10` and must be wired in before this
service is considered production-ready.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
