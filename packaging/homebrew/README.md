# Homebrew packaging

PLATFORM: macOS
STATUS: scaffolding; release URLs and SHA256s must be filled at release time.

## Release process

To cut a release:

1. Build and tag the release in git (`vX.Y.Z`).
2. Produce the source tarball via the GitHub release (auto-generated from the tag).
3. Compute the tarball SHA256 (`shasum -a 256 pcloud-rs-X.Y.Z.tar.gz`).
4. Update `url` and `sha256` placeholders in `pcloud-rs.rb`.
5. Submit a PR to `homebrew/homebrew-core` or the project tap with the updated formula.

No live publishing is performed from this repository. The formula in this
directory is the canonical scaffolding source; the actual tap copy lives
upstream.

## Runtime notes

- `fuse-t` is declared as a runtime cask because mounted-drive parity
  (`bd-1du.4`) requires a FUSE implementation on macOS.
- The launchd plist source lives under `packaging/macos/` and is copied into
  the formula prefix at install time for `brew services`.
