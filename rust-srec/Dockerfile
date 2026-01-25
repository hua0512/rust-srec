# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.92.0

# =============================================================================
# Stage 0: Chef - Rust toolchain + cargo-chef (shared base)
# =============================================================================
FROM rust:${RUST_VERSION}-bookworm AS chef
RUN cargo install --locked cargo-chef
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
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libssl3 \
    locales \
    python3 \
    python3-pip \
    unzip \
    xz-utils \
    && rm -rf /var/lib/apt/lists/* \
    && sed -i '/en_US.UTF-8/s/^# //g' /etc/locale.gen \
    && locale-gen

# Install ffmpeg with architecture check
RUN ARCH=$(uname -m) && \
    if [ "$ARCH" = "x86_64" ]; then \
      URL="https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz"; \
    elif [ "$ARCH" = "aarch64" ]; then \
      URL="https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linuxarm64-gpl.tar.xz"; \
    fi && \
    curl -L -o /tmp/ffmpeg.tar.xz "$URL" && \
    mkdir -p /tmp/ffmpeg && \
    tar -xJf /tmp/ffmpeg.tar.xz -C /tmp/ffmpeg --strip-components=1 && \
    mv /tmp/ffmpeg/bin/ffmpeg /usr/local/bin/ && \
    mv /tmp/ffmpeg/bin/ffprobe /usr/local/bin/ && \
    mv /tmp/ffmpeg/bin/ffplay /usr/local/bin/ && \
    chmod +x /usr/local/bin/ffmpeg /usr/local/bin/ffprobe /usr/local/bin/ffplay && \
    rm -rf /tmp/ffmpeg /tmp/ffmpeg.tar.xz

# Set UTF-8 locale
ENV LANG=en_US.UTF-8
ENV LANGUAGE=en_US:en
ENV LC_ALL=en_US.UTF-8

# Install Streamlink, ttvlol plugin, and rclone
RUN pip3 install --break-system-packages streamlink \
    && mkdir -p /root/.local/share/streamlink/plugins \
    && curl -L -o /root/.local/share/streamlink/plugins/twitch.py \
        'https://github.com/2bc4/streamlink-ttvlol/releases/latest/download/twitch.py' \
    && curl https://rclone.org/install.sh | bash \
    && arch=$(dpkg --print-architecture) \
    && if [ "$arch" = "amd64" ]; then \
        DANMAKU_ARCH="x86_64"; \
    elif [ "$arch" = "arm64" ]; then \
        DANMAKU_ARCH="arm64"; \
    else \
        echo "Unsupported architecture: $arch"; exit 1; \
    fi \
    && curl -L -o /tmp/danmaku.zip "https://github.com/renmu123/DanmakuFactory/releases/download/v2.0.0/DanmakuFactory-linux-${DANMAKU_ARCH}-CLI.zip" \
    && unzip /tmp/danmaku.zip -d /tmp/danmaku \
    && mv /tmp/danmaku/DanmakuFactory /usr/local/bin/DanmakuFactory \
    && chmod +x /usr/local/bin/DanmakuFactory \
    && rm -rf /tmp/danmaku.zip /tmp/danmaku

WORKDIR /app

# Copy the built binary from the builder stage
COPY --from=builder /app/target/release/rust-srec /app/rust-srec

# Expose the default port (adjust as needed)
EXPOSE 8080

# Run the application
CMD ["./rust-srec"]
