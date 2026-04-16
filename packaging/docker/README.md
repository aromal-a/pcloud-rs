<!-- PLATFORM: Linux (container runtime) -->
# pcloud-rs Docker image

Multi-arch container image for the Rust rewrite of pcloud-rs.

Supported platforms: `linux/amd64`, `linux/arm64`.

## Image layout

* Multi-stage build: `rust:1.82-bookworm` builder -> `debian:bookworm-slim` runtime.
* Non-root user `pcloud-rs` (uid/gid 1000) owns `$PCLOUDRS_HOME=/var/lib/pcloud-rs`.
* Entrypoint wrapped in `tini` for correct signal handling.
* OCI image labels populated (`org.opencontainers.image.*`).

## Build locally

From the repository root:

```bash
docker build \
    -f packaging/docker/Dockerfile \
    -t pcloud-rs:dev \
    
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
    ghcr.io/<owner>/pcloud-rs:latest
```

Drop `--device/--cap-add` if you only need API-level access (no mount).

## Run the CLI

```bash
docker run --rm -it ghcr.io/<owner>/pcloud-rs:latest pcloudc --help
```

## Multi-arch publishing

CI in `.github/workflows/docker.yml` pushes to `ghcr.io/<owner>/pcloud-rs`
on `main` pushes and release tags, covering `linux/amd64` and `linux/arm64`
via `docker/setup-qemu-action` + `docker/build-push-action`.
