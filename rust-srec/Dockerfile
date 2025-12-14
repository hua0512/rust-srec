# Build stage
FROM rust:1.91.1-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY rust-srec/Cargo.toml ./rust-srec/

# Create dummy main to cache dependencies
RUN mkdir -p rust-srec/src && \
    echo "fn main() {}" > rust-srec/src/main.rs && \
    cargo build --release --package rust-srec && \
    rm -rf rust-srec/src

# Copy actual source code
COPY rust-srec/src ./rust-srec/src

# Build the actual application
RUN touch rust-srec/src/main.rs && \
    cargo build --release --package rust-srec

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    ffmpeg \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -r -s /bin/false -m -d /app rust-srec

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/rust-srec /app/rust-srec

# Create data directories
RUN mkdir -p /app/data /app/config /app/output && \
    chown -R rust-srec:rust-srec /app

# Switch to non-root user
USER rust-srec

# Environment variables
ENV RUST_LOG=info
ENV DATABASE_URL=sqlite:///app/data/rust-srec.db
ENV OUTPUT_DIR=/app/output
ENV CONFIG_DIR=/app/config
ENV API_PORT=8080
ENV API_BIND_ADDRESS=0.0.0.0

# Expose API port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/api/health/ready || exit 1

# Run the application
ENTRYPOINT ["/app/rust-srec"]
CMD ["--config", "/app/config/config.toml"]
