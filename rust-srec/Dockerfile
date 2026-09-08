# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.95.0

# =============================================================================
# Stage 0: Chef - Rust toolchain + cargo-chef (shared base)
# =============================================================================
FROM rust:${RUST_VERSION}-bookworm AS chef
RUN cargo install --locked cargo-chef --version 0.1.78
WORKDIR /app

# =============================================================================
# Stage 1: Planner - Generate recipe.json (dependency manifest)
# =============================================================================
FROM chef AS planner

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY rust-srec ./rust-srec
COPY crates ./crates
COPY mesio-cli ./mesio-cli
COPY strev-cli ./strev-cli

# Limit the recipe to the backend binary.
# This avoids pulling in unrelated workspace members (e.g. the Tauri desktop crate).
RUN cargo chef prepare --recipe-path recipe.json --bin rust-srec

# =============================================================================
# Stage 2: Cacher - Build dependencies only (highly cacheable)
# =============================================================================
FROM chef AS cacher

# Install build dependencies including protoc.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        protobuf-compiler \
        pkg-config \
        libssl-dev \
        ; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=planner /app/recipe.json recipe.json

# Build scripts in this workspace compile protobufs; ensure proto files are available
# during dependency cooking.
COPY --from=planner /app/rust-srec/proto rust-srec/proto

# Build only dependencies - this layer is cached if recipe.json unchanged.
# We intentionally allow Cargo.lock updates here, then propagate the generated
# lockfile into the builder stage to keep the final build consistent.
RUN cargo chef cook --release --recipe-path recipe.json \
    --package rust-srec --bin rust-srec --features tls-native-fallback

# =============================================================================
# Stage 3: Builder - Build the actual application
# =============================================================================
FROM chef AS builder

# Install build dependencies including protoc.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        protobuf-compiler \
        pkg-config \
        libssl-dev \
        ; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy pre-built dependencies from cacher
COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY rust-srec ./rust-srec
COPY crates ./crates
COPY mesio-cli ./mesio-cli
COPY strev-cli ./strev-cli

# Use the lockfile produced by the cacher stage (may include target/feature-specific deps).
COPY --from=cacher /app/Cargo.lock Cargo.lock

# Build the release binary (only compiles our code, deps are cached).
# We first allow Cargo.lock to be updated (if needed for this target/feature set),
# then immediately re-run with --locked to ensure it is stable.
# Enable native-tls fallback for legacy TLS endpoints (e.g. Douyu CDN/danmu).
RUN set -eux; \
    cargo build --release --features tls-native-fallback -p rust-srec --bin rust-srec; \
    cargo build --locked --release --features tls-native-fallback -p rust-srec --bin rust-srec

# Runtime stage
FROM debian:bookworm-slim AS runtime

# Debian security updates remain available; downloaded tool artifacts are independently locked.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        ca-certificates curl libssl3 locales python3 python3-venv \
        libgcc-s1; \
    rm -rf /var/lib/apt/lists/*; \
    sed -i '/en_US.UTF-8/s/^# //g' /etc/locale.gen; \
    locale-gen

COPY rust-srec/docker/ /usr/local/share/rust-srec/image/
RUN set -eux; \
    python3 /usr/local/share/rust-srec/image/install-runtime-tools.py --architecture "$(dpkg --print-architecture)"; \
    python3 -m venv /opt/streamlink; \
    /opt/streamlink/bin/pip install --no-cache-dir --only-binary=:all: --require-hashes \
        --index-url https://pypi.org/simple \
        -r /usr/local/share/rust-srec/image/streamlink-requirements.txt; \
    /opt/streamlink/bin/pip check

ENV LANG=en_US.UTF-8 \
    LANGUAGE=en_US:en \
    LC_ALL=en_US.UTF-8 \
    NVIDIA_VISIBLE_DEVICES=all \
    NVIDIA_DRIVER_CAPABILITIES=compute,video,utility \
    HOME=/app/config \
    XDG_CONFIG_HOME=/app/config \
    XDG_DATA_HOME=/app/config \
    XDG_CACHE_HOME=/app/config/cache \
    BAIDUPCS_GO_CONFIG_DIR=/app/config/BaiduPCS-Go \
    DATABASE_URL=sqlite:///app/data/rust-srec.db \
    OUTPUT_DIR=/app/output \
    LOG_DIR=/app/logs \
    API_PORT=8080 \
    API_BIND_ADDRESS=0.0.0.0

RUN set -eux; \
    groupadd --gid 1000 recorder; \
    useradd --uid 1000 --gid 1000 --home-dir /app/config --no-create-home --shell /usr/sbin/nologin recorder; \
    mkdir -p /app/data /app/config/streamlink/plugins /app/config/cache /app/output /app/logs; \
    chown -R 1000:1000 /app

WORKDIR /app
COPY --from=builder --chmod=755 /app/target/release/rust-srec /app/rust-srec
COPY --chmod=755 rust-srec/scripts/watchtower-pre-update.sh /app/watchtower-pre-update.sh
COPY --chmod=755 rust-srec/docker/entrypoint.sh /usr/local/bin/rust-srec-entrypoint
COPY --chmod=755 rust-srec/docker/streamlink.sh /usr/local/bin/streamlink
COPY --chmod=755 rust-srec/docker/healthcheck.sh /usr/local/bin/rust-srec-healthcheck

USER 1000:1000
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 CMD ["rust-srec-healthcheck"]
ENTRYPOINT ["rust-srec-entrypoint"]
CMD ["/app/rust-srec"]
