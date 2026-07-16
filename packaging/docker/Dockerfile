# syntax=docker/dockerfile:1.7
#
# Multi-stage build for the pcloud-rs Rust daemon + CLI.
#
#   docker build -f packaging/docker/Dockerfile -t pcloud-rs:latest .
#
# Stage 1 is the only stage exercised in CI where docker is unavailable.
# Runtime is distroless/static: no shell, no package manager, read-only rootfs.

ARG RUST_VERSION=1.96.1
ARG DISTROLESS_IMAGE=gcr.io/distroless/static-debian12:nonroot

# -----------------------------------------------------------------------------
# Builder: glibc-hosted Rust toolchain cross-compiling a static musl target.
#
# Rust's Alpine image uses a musl *host* toolchain, which cannot build
# proc-macro dynamic libraries on the 1.96 line. A Debian host plus the musl
# target keeps proc macros host-native while the shipped binaries remain
# static and suitable for the distroless runtime.
# -----------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS builder

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        musl-tools \
        pkg-config \
        git \
 && rustup target add x86_64-unknown-linux-musl \
 && rm -rf /var/lib/apt/lists/*

ENV CARGO_TERM_COLOR=always \
    CARGO_BUILD_TARGET=x86_64-unknown-linux-musl \
    RUSTFLAGS="-C target-feature=+crt-static -C codegen-units=1 -C strip=symbols" \
    CARGO_NET_GIT_FETCH_WITH_CLI=true

WORKDIR /build
ARG GIT_SHA=unknown
COPY . .

# Build only the release binaries we ship. No tests, no examples.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    GIT_HASH="${GIT_SHA}" cargo build --release --locked \
        -p pcloud-daemon \
        -p pcloud-cli \
 && mkdir -p /out \
 && cp target/x86_64-unknown-linux-musl/release/pcloudd /out/pcloudd \
 && cp target/x86_64-unknown-linux-musl/release/pcloudc /out/pcloudc \
 && strip /out/pcloudd /out/pcloudc || true

# -----------------------------------------------------------------------------
# Runtime: distroless static, nonroot (uid 65532).
# -----------------------------------------------------------------------------
FROM ${DISTROLESS_IMAGE} AS runtime

ARG GIT_SHA=unknown
LABEL org.opencontainers.image.title="pcloud-rs" \
      org.opencontainers.image.description="pCloud console client (Rust)" \
      org.opencontainers.image.source="https://github.com/ezechiel203/pcloud-rs" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0" \
      org.opencontainers.image.revision="${GIT_SHA}"

# distroless nonroot: uid/gid = 65532
USER 65532:65532

COPY --from=builder --chown=65532:65532 /out/pcloudd /usr/local/bin/pcloudd
COPY --from=builder --chown=65532:65532 /out/pcloudc /usr/local/bin/pcloudc

# State, config, runtime socket, and cache live under PCLOUD_ROOT.
# Compose / k8s must mount this as a 0700 volume owned by uid/gid 65532.
VOLUME ["/var/lib/pcloud-rs"]

ENV PCLOUD_ROOT=/var/lib/pcloud-rs \
    PCLOUD_ENV=production \
    RUST_LOG=info

# IPC-based healthcheck: CLI derives the owner-only socket from PCLOUD_ROOT.
# No network probe, no shell required.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/pcloudc", "health"]

ENTRYPOINT ["/usr/local/bin/pcloudd"]
CMD ["serve"]
