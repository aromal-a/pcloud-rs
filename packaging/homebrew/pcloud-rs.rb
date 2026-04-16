# PLATFORM: macOS
# STATUS: scaffolding; release URLs and SHA256s must be filled at release time.
#
# Homebrew formula for pcloud-rs (Rust rewrite).
# See packaging/homebrew/README.md for the release process.
#
# Binary naming: `cargo install` picks the `[[bin]]` name from each crate's
# Cargo.toml; for this project that is `pcloudc` (from pcloud-cli) and
# `pcloudd` (from pcloud-daemon). The `service` stanza therefore references
# `opt_bin/"pcloudd"`, not a dashed variant like `pcloud-daemon`.

class Pcloudcc < Formula
  desc "pCloud command-line client and sync daemon (Rust rewrite)"
  homepage "https://github.com/pcloudcom/pcloud-rs"
  url "https://github.com/pcloudcom/pcloud-rs/archive/refs/tags/vX.Y.Z.tar.gz"
  sha256 "SHA256_PLACEHOLDER"
  license any_of: ["MIT", "Apache-2.0"]

  depends_on "rust" => :build
  depends_on "fuse-t" => :runtime

  def install
    system "cargo", "install",
           "--locked",
           "--root", prefix,
           "--path", "crates/pcloud-cli"

    system "cargo", "install",
           "--locked",
           "--root", prefix,
           "--path", "crates/pcloud-daemon"

    plist_src = "packaging/macos/homebrew.mxcl.pcloud-rs.plist"
    if File.exist?(plist_src)
      (prefix/"homebrew.mxcl.pcloud-rs.plist").write(File.read(plist_src))
    end
  end

  service do
    # `pcloudd` is the binary name declared in
    # crates/pcloud-daemon/Cargo.toml `[[bin]] name = "pcloudd"`.
    # --foreground keeps the process in the foreground so `brew services`
    # can supervise it directly (brew invokes launchctl under the hood).
    run [opt_bin/"pcloudd", "--foreground"]
    keep_alive true
    log_path var/"log/pcloud-rs.log"
    error_log_path var/"log/pcloud-rs.err.log"
  end

  test do
    system "#{bin}/pcloudc", "--version"
  end
end
