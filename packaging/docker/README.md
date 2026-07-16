<!-- PLATFORM: Linux (container runtime) -->
# pcloud-rs Docker image

Local container image recipe for the Rust rewrite of pcloud-rs.

Supported local target: Linux OCI runtimes. The authoritative image gate is
`cargo xtask docker`; registry publishing and signing remain
operator-controlled release actions.

## Image layout

* Multi-stage build: `rust:1.96.1-bookworm` host toolchain, static musl
  cross-target -> distroless static runtime.
* Non-root distroless user `65532:65532` owns `PCLOUD_ROOT=/var/lib/pcloud-rs`.
* Entrypoint is `pcloudd`; default command is `serve`.
* OCI labels point at this repository and the workspace `MIT OR Apache-2.0` license.

## Build locally

From the repository root:

```bash
cargo xtask docker
```

For a direct development build:

```bash
docker build \
    -f packaging/docker/Dockerfile \
    -t pcloud-rs:dev \
    .
```

## Run the daemon

The daemon needs `/dev/fuse` and `CAP_SYS_ADMIN` to mount the cloud drive:

```bash
docker run --rm -it \
    --name pcloudd \
    --device /dev/fuse \
    --cap-add SYS_ADMIN \
    --security-opt apparmor=unconfined \
    -v pcloud-rs-state:/var/lib/pcloud-rs \
    pcloud-rs:dev
```

Drop `--device/--cap-add` if you only need API-level access (no mount).

## Secret bootstrap

Do not put pCloud passwords or tokens directly in `environment:`. Mount
owner-only secret files and point the daemon at those paths:

```bash
docker run --rm -it \
    --name pcloudd \
    -v "$PWD/state:/var/lib/pcloud-rs" \
    -v "$PWD/secrets/pcloud-rs-token:/run/secrets/pcloud-rs-token:ro" \
    -e PCLOUD_ROOT=/var/lib/pcloud-rs \
    -e PCLOUDRS_TOKEN_FILE=/run/secrets/pcloud-rs-token \
    pcloud-rs:dev
```

For first-boot username/password bootstrap, use
`PCLOUDRS_USERNAME_FILE` and `PCLOUDRS_PASSWORD_FILE`; add exactly one of
`PCLOUDRS_TFA_CODE_FILE` or `PCLOUDRS_RECOVERY_CODE_FILE` when second
factor is required. `PCLOUDRS_TRUST_DEVICE=1` requests a trusted-device
login after successful TFA.

## Run the CLI

```bash
docker run --rm -it \
    --entrypoint /usr/local/bin/pcloudc \
    pcloud-rs:dev --help
```

## Publishing

GitHub Actions is intentionally disabled, so the repository does not
automatically publish GHCR images or cosign signatures. `cargo xtask docker`
builds and smokes the local image; `cargo xtask release` runs the complete
local release gate before an operator performs registry authentication,
multi-architecture publication, and signing.
