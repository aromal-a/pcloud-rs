# pcloud-daemon-win

**Status:** experimental and unshipped. The public Windows MSI does not install
this SCM host. Supported Windows operation uses the regular `pcloudd.exe`,
started per-user by `pcloudc start`, so named-pipe authentication, DPAPI, and
WinFSP all share the interactive user's SID.

**PLATFORM:** Windows only. On Linux/macOS/FreeBSD this crate compiles to a
no-op binary.

`pcloudd-svc.exe` is a thin Windows Service wrapper around the `pcloud-daemon`
runtime. It registers with the Service Control Manager (SCM), translates
`Stop` / `Shutdown` signals into a cooperative shutdown flag, and hosts the
daemon worker inside the SCM-managed process.

This crate links the real cross-platform daemon runtime from
`crates/pcloud-daemon`; it does not contain a second IPC or dispatch
implementation.

## Development-only manual experiment

Do not use this procedure for public or production installs. On an isolated
test host, an administrator can register a locally built binary:

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

* `pcloud-daemon` — the actual daemon runtime, linked into this wrapper.
* `windows-service` crate (0.7) — SCM FFI bindings.

## Status

The wrapper invokes `pcloud_daemon::serve_with_shutdown` and maps SCM
`Stop`/`Shutdown` to its cooperative flag. It remains intentionally outside
the supported package: a machine/service-account process does not satisfy the
released per-user SID ownership model. Native SCM install/start/stop testing is
therefore an experimental qualification gate, not evidence for the public
Windows path.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
