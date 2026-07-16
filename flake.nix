{
  description = "pcloud-rs — Rust rewrite () + legacy C dev shell";

  inputs = {
    # Pinned to the exact nixpkgs revision that flake.lock already records,
    # so `nix build` without --refresh is deterministic even for consumers
    # who do not trust the lock file. Bump this rev together with flake.lock.
    # See docs/book/src/development/reproducible-builds.md §4.5.
    nixpkgs.url = "github:NixOS/nixpkgs/f675531bc7e6657c10a18b565cfebd8aa9e24c14";
    flake-utils.url = "github:numtide/flake-utils";
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, crane }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        craneLib = crane.mkLib pkgs;

        src = craneLib.cleanCargoSource ./;

        commonArgs = {
          inherit src;
          strictDeps = true;
          pname = "pcloud-rs";
          version = "0.1.0";

          nativeBuildInputs = with pkgs; [ pkg-config ];

          buildInputs = with pkgs; [
            openssl
            sqlite
            zlib
          ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            fuse3
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.libiconv
          ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        pcloud-rs = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          doCheck = false;
          meta = {
            description = "pCloud command-line client (Rust rewrite)";
            license = [ pkgs.lib.licenses.mit pkgs.lib.licenses.asl20 ];
            mainProgram = "pcloudc";
          };
        });

        # Reproducible-build derivation.
        #
        # Uses the `release-repro` profile pinned in Cargo.toml and
        # the SOURCE_DATE_EPOCH + --remap-path-prefix + --build-id=none
        # contract documented in docs/book/src/development/reproducible-builds.md.
        #
        # Consumers: `nix build .#pcloud-rs-repro` reproduces the same
        # release-repro profile and deterministic RUSTFLAGS. It is not
        # byte-identical to the signed GitHub release asset until the Nix
        # build path is switched to cargo-auditable as well.
        pcloud-rs-repro = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "pcloud-rs-repro";
          doCheck = false;
          cargoExtraArgs = "--locked --profile release-repro -p pcloud-cli -p pcloud-daemon";
          CARGO_PROFILE_RELEASE_REPRO_ACTIVE = "1";
          # Nix already gives us a sandboxed, timestamp-pinned build; still,
          # export SOURCE_DATE_EPOCH=1 explicitly so rustc honours it the same
          # way it does under CI.
          SOURCE_DATE_EPOCH = "1";
          RUSTFLAGS = "--remap-path-prefix=/build/source= -C link-arg=-Wl,--build-id=none";
          meta = {
            description = "pCloud client — reproducible build (release-repro profile)";
            license = [ pkgs.lib.licenses.mit pkgs.lib.licenses.asl20 ];
            mainProgram = "pcloudc";
          };
        });
      in
      {
        packages = {
          default = pcloud-rs;
          pcloud-rs = pcloud-rs;
          pcloudc = pcloud-rs;
          pcloudd = pcloud-rs;
          pcloud-rs-repro = pcloud-rs-repro;
        };

        apps = {
          default = flake-utils.lib.mkApp {
            drv = pcloud-rs;
            name = "pcloudc";
          };
          pcloudc = flake-utils.lib.mkApp {
            drv = pcloud-rs;
            name = "pcloudc";
          };
          pcloudd = flake-utils.lib.mkApp {
            drv = pcloud-rs;
            name = "pcloudd";
          };
        };

        devShells = {
          # Rust rewrite dev shell: rust-toolchain + pkg-config + libfuse3 +
          # fuse-overlayfs (Linux only), plus the build inputs needed by the workspace.
          default = pkgs.mkShell {
            inputsFrom = [ pcloud-rs ];
            packages = with pkgs; [
              rustc
              cargo
              clippy
              rustfmt
              rust-analyzer
              pkg-config
              openssl
              sqlite
              cargo-deny
              cargo-audit
            ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              fuse3
              fuse-overlayfs
            ];
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };

          # Legacy C/C++ dev shell (preserved from the previous flake).
          legacy-c = pkgs.mkShell {
            packages = with pkgs; [
              bear
              clang-tools
              zlib
              sqlite
              boost
              libudev-zero
              readline
              fuse
              mbedtls
              watchexec
            ];
          };
        };

        checks = {
          inherit pcloud-rs pcloud-rs-repro;
          fmt = craneLib.cargoFmt {
            inherit src;
          };
          clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--workspace --all-targets -- -D warnings";
          });
          test = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            cargoExtraArgs = "--workspace";
          });
        };
      });
}
