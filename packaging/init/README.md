# Cross-Init Service Scripts

This directory contains reference service definitions for the Rust daemon
across common Linux and BSD init systems.

All scripts use the same non-interactive credential contract:

- preferred: `PCLOUDRS_TOKEN_FILE`
- first-boot fallback:
  - `PCLOUDRS_USERNAME_FILE`
  - `PCLOUDRS_PASSWORD_FILE`
  - optional `PCLOUDRS_TFA_CODE_FILE` or `PCLOUDRS_RECOVERY_CODE_FILE`

No script prompts for input. The daemon itself now consumes these files during
bootstrap and can authenticate before serving requests.

Use the example env file in:

- `common/pcloudd.env.example`

Install the wrapper script to:

- `/usr/local/libexec/pcloudd-wrapper.sh`

Then install the init-system-specific file that matches your platform.

The scripts in this directory cover:

- `systemd`
- `sysvinit`
- `OpenRC`
- `runit`
- `s6`
- `dinit`
- `FreeBSD rc.d`
- `NetBSD rc.d`
- `OpenBSD rc.d`

DragonFly BSD uses the same `rc.d` model as FreeBSD for this daemon; the
FreeBSD script is the intended starting point there.
