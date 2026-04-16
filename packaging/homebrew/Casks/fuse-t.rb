# PLATFORM: macOS (Homebrew Cask)
# STATUS: scaffolding
#
# Documentation-only fallback cask for fuse-t, the macFUSE-compatible
# FUSE runtime that pcloud-rs's mounted-drive feature depends on.
#
# NOTE: An *official* fuse-t cask may already be published by the
# fuse-t project (check `brew search fuse-t` and the macos-fuse-t
# Homebrew tap). If the official cask is available, users should
# prefer it. This file exists as a pinned fallback for fork builds
# where the upstream tap is not reachable or has not yet been
# approved into homebrew-cask.
#
# Placeholders (url / sha256 / version) must be updated to a real
# release before this cask is used. Running `brew install --cask`
# against this file as-is will fail the SHA256 check by design.

cask "fuse-t" do
  version "0.0.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/macos-fuse-t/fuse-t/releases/download/#{version}/fuse-t-macos-#{version}.pkg"
  name "FUSE-T"
  desc "macFUSE-compatible FUSE implementation for macOS (NFS-backed)"
  homepage "https://www.fuse-t.org/"

  pkg "fuse-t-macos-#{version}.pkg"

  uninstall pkgutil: [
    "io.fuse-t.pkg.fuse-t",
  ]

  caveats <<~EOS
    fuse-t is required for the pcloud-rs mounted-drive feature on macOS.
    If an official fuse-t cask is available via `brew search fuse-t`,
    prefer it over this fallback.
  EOS
end
