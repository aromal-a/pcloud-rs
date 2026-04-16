# pcloud-rs manpages

This directory ships the canonical manpages for the Rust rewrite:

- `pcloudc.1` — user-facing CLI reference
- `pcloudd.1` — daemon reference
- `pcloud.conf.5` — configuration file format

## Installing

Packagers should install the pages under `$(mandir)/man1` and
`$(mandir)/man5`:

```
install -Dm0644 pcloudc.1     $(DESTDIR)/usr/share/man/man1/pcloudc.1
install -Dm0644 pcloudd.1     $(DESTDIR)/usr/share/man/man1/pcloudd.1
install -Dm0644 pcloud.conf.5 $(DESTDIR)/usr/share/man/man5/pcloud.conf.5
```

## Linting

All pages must pass `mandoc -T lint` cleanly. CI enforces this via the
`manpage-lint` job in `.github/workflows/rust.yml`. To lint locally:

```
mandoc -T lint packaging/man/*.1 packaging/man/*.5
```

### `mandoc` availability by distro

| Distribution          | `mandoc` preinstalled?         | How to install            |
|-----------------------|--------------------------------|---------------------------|
| OpenBSD               | Yes (base system)              | —                         |
| NetBSD                | Yes (base system, 7.0+)        | —                         |
| FreeBSD               | Yes (base system, 10.0+)       | —                         |
| Alpine Linux          | Yes (`mandoc` replaces `man`)  | `apk add mandoc`          |
| Void Linux            | Yes (default `MANPAGER`)       | `xbps-install mandoc`     |
| Arch Linux            | No (provides `man-db`)         | `pacman -S mandoc`        |
| Debian / Ubuntu       | No (provides `man-db`)         | `apt install mandoc`      |
| Fedora / RHEL / CentOS| No                             | `dnf install mandoc`      |
| openSUSE              | No                             | `zypper install mandoc`   |
| macOS                 | Yes (base system uses mandoc)  | —                         |

GitHub Actions' `ubuntu-latest` image does NOT include `mandoc` by
default, so the `manpage-lint` job installs it via `apt`.

## Previewing

```
man -l packaging/man/pcloudc.1
man -l packaging/man/pcloudd.1
man -l packaging/man/pcloud.conf.5
```

## Source of truth

- CLI command list:
  `crates/pcloud-cli/src/commands.rs`
- Exit-code table:
  `crates/pcloud-cli/src/exit_code.rs`
- Environment-variable list: grep `PCLOUD_` across `crates`.

When the CLI surface or an exit code changes, the corresponding
manpage section must be updated in the same PR.
