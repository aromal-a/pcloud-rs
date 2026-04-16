# Chocolatey packaging

PLATFORM: Windows
STATUS: scaffolding; release URLs and SHA256s must be filled at release time.

## Release process

To cut a release:

1. Build the MSI installer via WiX from the release branch.
2. Upload the MSI asset to the GitHub release for `vX.Y.Z`.
3. Update the `url64`, `checksum64`, and `<version>` fields in
   `tools/chocolateyinstall.ps1` and `pcloud-rs.nuspec`.
4. Run `choco pack` locally to validate, then submit a PR to the
   community Chocolatey repository (`chocolatey-community/chocolatey-packages`)
   or push via `choco push` from the maintainers' account.

No live publishing steps are executed from this repository. These files are
source-of-truth scaffolding; the production feed copy lives upstream.

## Dependencies

- `winfsp` is a hard runtime dependency because Windows mounted-drive parity
  (`bd-1du.4`) depends on WinFsp.
