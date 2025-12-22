# Build stage
FROM rust:1.92.0-bookworm AS builder

# Install build dependencies including protoc
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy workspace files
COPY Cargo.toml Cargo.lock ./

# Copy required workspace members only
COPY rust-srec ./rust-srec
COPY crates ./crates
COPY mesio-cli ./mesio-cli
COPY strev-cli ./strev-cli

# Build the release binary
RUN cargo build --release --bin rust-srec

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    ffmpeg \
    python3 \
    python3-pip \
    unzip \
    && rm -rf /var/lib/apt/lists/*

# Install Streamlink, ttvlol plugin, and rclone
RUN pip3 install --break-system-packages streamlink \
    && mkdir -p /root/.local/share/streamlink/plugins \
    && curl -L -o /root/.local/share/streamlink/plugins/twitch.py \
        'https://github.com/2bc4/streamlink-ttvlol/releases/latest/download/twitch.py' \
    && curl https://rclone.org/install.sh | bash

WORKDIR /app

# Copy the built binary from the builder stage
COPY --from=builder /app/target/release/rust-srec /app/rust-srec

# Expose the default port (adjust as needed)
EXPOSE 8080

# Run the application
CMD ["./rust-srec"]
