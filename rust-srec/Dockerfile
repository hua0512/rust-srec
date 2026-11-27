# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.95.0
ARG CARGO_PROFILE=release

# =============================================================================
# Stage 0: Chef - Rust toolchain + prebuilt cargo-chef (shared base)
# =============================================================================
FROM lukemathwalker/cargo-chef:latest-rust-${RUST_VERSION}-bookworm AS chef
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

# Install build dependencies once so both cacher and builder reuse this layer
# across dev and release profiles.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=planner /app/recipe.json recipe.json

ARG CARGO_PROFILE

# Build only dependencies - this layer is cached if recipe.json unchanged.
# The recipe has a reduced workspace and lockfile; keep any lockfile updates
# confined to this dependency cache rather than the final application build.
RUN cargo chef cook --profile "${CARGO_PROFILE}" --recipe-path recipe.json \
    --package rust-srec --bin rust-srec --features tls-native-fallback

# =============================================================================
# Stage 3: Builder - Build the actual application binaries
# =============================================================================
# Inherit directly from cacher so /app/target and /usr/local/cargo stay in place
# without duplicating multi-GB layers into the BuildKit registry cache.
FROM cacher AS builder

ARG CARGO_PROFILE

WORKDIR /app

# Replace cargo-chef's skeleton sources and reduced lockfile with the real workspace.
RUN rm -rf Cargo.toml Cargo.lock recipe.json rust-srec crates mesio-cli strev-cli

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY rust-srec ./rust-srec
COPY crates ./crates
COPY mesio-cli ./mesio-cli
COPY strev-cli ./strev-cli

# Build both release binaries in one pass so the `vapid` target reuses this stage
# instead of compiling the workspace dependency graph a second time.
# Enable native-tls fallback for legacy TLS endpoints (e.g. Douyu CDN/danmu).
RUN cargo build --locked --profile "${CARGO_PROFILE}" --features tls-native-fallback -p rust-srec --bin rust-srec --bin rust-srec-vapid \
    && mkdir -p /out \
    && cp "target/${CARGO_PROFILE}/rust-srec" "target/${CARGO_PROFILE}/rust-srec-vapid" /out/

# =============================================================================
# Optional target: tiny rust-srec-vapid image (`--target vapid`)
# =============================================================================
FROM debian:bookworm-slim AS vapid

WORKDIR /app

COPY --from=builder /out/rust-srec-vapid /app/rust-srec-vapid

ENTRYPOINT ["./rust-srec-vapid"]

# Runtime stage (last stage remains the default build target)
FROM debian:bookworm-slim AS runtime

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

# NVIDIA Container Toolkit support: when the container runs under the toolkit,
# these env vars tell it which driver capabilities to expose.
# Has no effect when no NVIDIA GPU is present.
ENV NVIDIA_VISIBLE_DEVICES=all
ENV NVIDIA_DRIVER_CAPABILITIES=compute,video,utility

# Install Streamlink, ttvlol plugin, rclone, rship, DanmakuFactory, and BaiduPCS-Go
RUN pip3 install --break-system-packages streamlink \
    && mkdir -p /root/.local/share/streamlink/plugins \
    && curl -L -o /root/.local/share/streamlink/plugins/twitch.py \
        'https://github.com/2bc4/streamlink-ttvlol/releases/latest/download/twitch.py' \
    && curl https://rclone.org/install.sh | bash \
    && curl -fsSL https://hua0512.github.io/rship/install.sh | sh \
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
    && rm -rf /tmp/danmaku.zip /tmp/danmaku \
    && curl -L -o /tmp/baidupcs.zip "https://github.com/qjfoidnh/BaiduPCS-Go/releases/download/v4.0.1/BaiduPCS-Go-v4.0.1-linux-${arch}.zip" \
    && unzip /tmp/baidupcs.zip -d /tmp/baidupcs \
    && mv /tmp/baidupcs/*/BaiduPCS-Go /usr/local/bin/BaiduPCS-Go \
    && chmod +x /usr/local/bin/BaiduPCS-Go \
    && rm -rf /tmp/baidupcs.zip /tmp/baidupcs

# Keep external uploader configs, OAuth tokens, and resume checkpoints inside
# the mounted config volume; default locations under $HOME would be lost on
# container recreation.
ENV BAIDUPCS_GO_CONFIG_DIR=/app/config/BaiduPCS-Go
ENV RSHIP_CONFIG=/app/config/rship/config.toml
ENV RSHIP_STATE_DIR=/app/config/rship/state

WORKDIR /app

# Copy the built binary from the builder stage
COPY --from=builder /out/rust-srec /app/rust-srec

# Watchtower pre-update hook: gates container auto-updates on /api/health/idle.
COPY --chmod=755 rust-srec/scripts/watchtower-pre-update.sh /app/watchtower-pre-update.sh

# Expose the default port (adjust as needed)
EXPOSE 8080

# Run the application
CMD ["./rust-srec"]
