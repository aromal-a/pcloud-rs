# syntax=docker/dockerfile:1.7
#
# Multi-stage build for the pcloud-rs Rust daemon + CLI.
#
#   docker build -f packaging/docker/Dockerfile -t pcloud-rs:latest 
#
# Stage 1 is the only stage exercised in CI where docker is unavailable.
# Runtime is distroless/static: no shell, no package manager, read-only rootfs.

ARG RUST_VERSION=1.82
ARG ALPINE_VERSION=3.20
ARG DISTROLESS_IMAGE=gcr.io/distroless/static-debian12:nonroot

# -----------------------------------------------------------------------------
# Builder: rust:alpine, static musl target.
# -----------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS builder

RUN apk add --no-cache \
        build-base \
        musl-dev \
        pkgconf \
        openssl-dev \
        openssl-libs-static \
        sqlite-dev \
        sqlite-static \
        fuse3-dev \
        linux-headers \
        git

ENV CARGO_TERM_COLOR=always \
    RUSTFLAGS="-C target-feature=+crt-static -C codegen-units=1 -C strip=symbols" \
    CARGO_NET_GIT_FETCH_WITH_CLI=true

WORKDIR /build
COPY . .

# Build only the release binaries we ship. No tests, no examples.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked \
        -p pcloud-daemon \
        -p pcloud-cli \
 && mkdir -p /out \
 && cp target/release/pcloudd /out/pcloudc-daemon \
 && cp target/release/pcloudc    /out/pcloudc \
 && strip /out/pcloudc-daemon /out/pcloudc || true

# -----------------------------------------------------------------------------
# Runtime: distroless static, nonroot (uid 65532).
# -----------------------------------------------------------------------------
FROM ${DISTROLESS_IMAGE} AS runtime

ARG GIT_SHA=unknown
LABEL org.opencontainers.image.title="pcloud-rs" \
      org.opencontainers.image.description="pCloud console client (Rust)" \
      org.opencontainers.image.source="https://github.com/pcloudcom/console-client" \
      org.opencontainers.image.licenses="GPL-3.0-or-later" \
      org.opencontainers.image.revision="${GIT_SHA}"

# distroless nonroot: uid/gid = 65532
USER 65532:65532

COPY --from=builder --chown=65532:65532 /out/pcloudc-daemon /usr/local/bin/pcloudd
COPY --from=builder --chown=65532:65532 /out/pcloudc        /usr/local/bin/pcloudc

# State + socket dir. Compose / k8s must mount a 0700 volume here.
VOLUME ["/var/lib/pcloud-rs", "/run/pcloud-rs"]

ENV PCLOUDRS_STATE_DIR=/var/lib/pcloud-rs \
    PCLOUDRS_RUNTIME_DIR=/run/pcloud-rs \
    PCLOUDRS_SOCKET=/run/pcloud-rs/daemon.sock

# IPC-based healthcheck: CLI pings the owner-only socket. No network probe,
# no shell required (CLI is a static binary and exits non-zero on failure).
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/pcloudc", "--socket", "/run/pcloud-rs/daemon.sock", "ping"]

ENTRYPOINT ["/usr/local/bin/pcloudd"]
CMD ["--socket", "/run/pcloud-rs/daemon.sock"]
