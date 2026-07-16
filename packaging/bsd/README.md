# BSD packaging and service assets

All explicitly supported BSD targets have native service definitions and
native VM runtime/mount gates. The repository contains service assets, not
downstream ports/pkgsrc packages; a retained release-commit job and native
install/upgrade test remain required before making a package support claim.

| Platform | Service asset | Binary prefix | Lifecycle model |
|---|---|---|---|
| FreeBSD | `packaging/freebsd/pcloudd.rc` | `/usr/local` | `daemon(8)` supervisor PID, restart delay, SIGTERM forwarding |
| DragonFly BSD | `packaging/dragonfly/pcloudd` | `/usr/local` | native `daemon(8)` supervisor PID, restart delay, SIGTERM forwarding |
| NetBSD | `packaging/netbsd/pcloudd` | `/usr/pkg` | `rc.subr` background start with the exec-preserved child PID |
| OpenBSD | `packaging/openbsd/pcloudd` | `/usr/local` | documented `rc_bg=YES`, `pgrep`/`pkill` process matching |

Every definition runs as a dedicated unprivileged identity, stores state below
`/var/lib/pcloud-rs` with mode `0700`, launches the required `pcloudd serve`
foreground command through `pcloudd-wrapper.sh`, and sends SIGTERM during
normal shutdown. The wrapper accepts only credential *file paths*; it never
places a token or password on the command line.

The service environment file should be administrator-owned and readable by
the service identity but not writable by it (for example root/service-group
`0640`). Credential files referenced by it must remain owner-only `0600`.

Use the platform page in the mdBook and the README adjacent to an asset for
identity creation and enable/start commands. `packaging/unix/validate.sh`
checks shell syntax, SMF XML syntax, archive content checksums, and deterministic
candidate reconstruction on Linux; native jobs perform the OS-specific gates.
