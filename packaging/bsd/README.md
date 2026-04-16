# BSD packaging

rc.d init scripts for running `pcloudd` as a system service on FreeBSD,
OpenBSD, and NetBSD. Each script is intentionally minimal; package-build
glue (pkg-plist, Makefiles, pkgsrc) is out of scope here.

> Note: BSD support for the Rust rewrite is still compile-gap. These
> scripts describe the intended deployment shape; they do not imply the
> Rust daemon currently builds cleanly on *BSD. See `PLAN_CROSSPLATFORM.md`.

## FreeBSD

Expected binary path: `/usr/local/bin/pcloudd`

```sh
install -m 0555 packaging/freebsd/pcloudd.rc /usr/local/etc/rc.d/pcloudd
sysrc pcloudd_enable="YES"
service pcloudd start
```

Tunables in `/etc/rc.conf`:

- `pcloudd_enable="YES"` — enable at boot
- `pcloudd_user="pcloud"` — run-as user
- `pcloudd_flags=""` — extra CLI flags

## OpenBSD

Expected binary path: `/usr/local/bin/pcloudd`

```sh
install -m 0555 packaging/openbsd/pcloudd /etc/rc.d/pcloudd
rcctl enable pcloudd
rcctl start pcloudd
```

Override flags with `rcctl set pcloudd flags "..."`.
The script runs as the dedicated `_pcloud` user (create with `useradd -r ...`).

## NetBSD

Expected binary path: `/usr/pkg/bin/pcloudd`

```sh
install -m 0555 packaging/netbsd/pcloudd /etc/rc.d/pcloudd
echo 'pcloudd=YES' >> /etc/rc.conf
/etc/rc.d/pcloudd start
```

PID file lives at `/var/run/pcloudd.pid`. Use `/etc/rc.d/pcloudd status`
to check, `stop` to terminate, `restart` to cycle.

## Shared notes

- All three scripts assume `pcloudd` honors a `-p <pidfile>` flag. Adjust
  `command_args` / `daemon_flags` if the real binary uses a different
  option name.
- Log output should go to syslog; none of the scripts redirect stdout.
- State directory (`$HOME/.pcloud` equivalent) is user-scoped; these
  scripts do not pre-create it.
