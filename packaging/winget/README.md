# winget packaging

PLATFORM: Windows
STATUS: scaffolding; release URLs and SHA256s must be filled at release time.

## Release process

To cut a release:

1. Build the MSI installer via WiX.
2. Upload the MSI to the GitHub release for `vX.Y.Z`.
3. Compute the SHA256 of the MSI (`Get-FileHash -Algorithm SHA256`).
4. Update `PackageVersion`, `InstallerUrl`, `InstallerSha256`, and
   `ProductCode` in `pcloud-rs.yaml`.
5. Split `pcloud-rs.yaml` into the three manifest files required by winget
   (`version`, `defaultLocale`, `installer`) under
   `manifests/p/pCloud/pcloud-rs/0.1.0/` in a fork of
   `microsoft/winget-pkgs`, and submit the PR upstream.

No live publishing steps are executed from this repository. The merged
`pcloud-rs.yaml` is the canonical source; the upstream community repository
holds the split copies that winget actually consumes.

## Notes

- `Scope: machine` reflects that the daemon registers a Windows service.
- WinFsp is installed separately; winget will not pull it as a dependency
  because winget manifest v1.5.0 does not model hard package dependencies
  the way Chocolatey does. Document WinFsp prerequisite in release notes.
