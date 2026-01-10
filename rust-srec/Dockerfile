# =============================================================================
# Stage 1: Planner - Generate recipe.json (dependency manifest)
# =============================================================================
FROM rust:1.92.0-bookworm AS planner

RUN cargo install cargo-chef

WORKDIR /app

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY rust-srec ./rust-srec
COPY crates ./crates
COPY mesio-cli ./mesio-cli
COPY strev-cli ./strev-cli

RUN cargo chef prepare --recipe-path recipe.json

# =============================================================================
# Stage 2: Cacher - Build dependencies only (highly cacheable)
# =============================================================================
FROM rust:1.92.0-bookworm AS cacher

RUN cargo install cargo-chef

# Install build dependencies including protoc
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=planner /app/recipe.json recipe.json

# Build only dependencies - this layer is cached if recipe.json unchanged
RUN cargo chef cook --release --recipe-path recipe.json

# =============================================================================
# Stage 3: Builder - Build the actual application
# =============================================================================
FROM rust:1.92.0-bookworm AS builder

# Install build dependencies including protoc
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

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

# Build the release binary (only compiles our code, deps are cached)
RUN cargo build --release --bin rust-srec

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
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
