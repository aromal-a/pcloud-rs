<!--
PLATFORM: Windows (Scoop)
STATUS: scaffolding
-->

# pcloud-rs Scoop packaging

This directory contains a scaffolding Scoop manifest
(`pcloud-rs.json`) for installing the Rust rewrite of `pcloud-rs` on
Windows via [Scoop](https://scoop.sh/).

## Local install (testing)

```powershell
scoop install .\packaging\scoop\pcloud-rs.json
```

The manifest expects a published release ZIP containing
`pcloudc.exe` and `pcloudd.exe` and pins a SHA256. Update `version`,
`url`, and `hash` for each release.

## WinFSP dependency

The manifest declares `"depends": "winfsp"`. Scoop will install the
WinFSP userspace bits automatically, but **WinFSP ships a
kernel-mode filesystem driver**:

- the driver install may require an elevated UAC prompt,
- on first install, a reboot is sometimes required before `pcloudd`
  can successfully mount a drive,
- corporate-managed systems may block the driver install entirely.

A `pre_install` hook in the manifest prints a yellow warning at
install time so users are not surprised.

## Distribution channels

There are two publication paths:

### 1. Submit to ScoopInstaller/Main

Open a PR against
[ScoopInstaller/Main](https://github.com/ScoopInstaller/Main)
adding `bucket/pcloud-rs.json`. Upstream requires:

- stable release URLs (GitHub Releases are fine),
- verified SHA256,
- a working `checkver` / `autoupdate` block (already scaffolded).

This is the recommended path for wide reach, but submission is
gated on upstream maintainer review.

### 2. Maintain a private Scoop bucket

Publish a Git repo (for example `your-org/scoop-bucket`) and tell
users:

```powershell
scoop bucket add pcloud-rs https://github.com/your-org/scoop-bucket
scoop install pcloud-rs/pcloud-rs
```

This path has no external review and is appropriate for fork
builds, prerelease channels, or enterprise-internal distribution.

## Status

This is scaffolding. No bucket submission has been made and the
manifest placeholders are not yet bound to a real release artifact.
