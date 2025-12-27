# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

# Multi-stage build for StreamKit server (slim image)
# Models and plugins should be mounted externally
# syntax=docker/dockerfile:1

# Stage 1: Build Rust dependencies
FROM rust:1.92-slim-bookworm AS rust-deps

WORKDIR /build

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    g++ \
    cmake \
    libopus-dev \
    libclang-dev \
    clang \
    curl \
    git \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files to build dependencies
COPY Cargo.toml Cargo.lock ./
COPY apps ./apps
COPY crates ./crates
COPY sdks ./sdks
COPY wit ./wit

# Create dummy ui/dist directory so server's RustEmbed doesn't fail
# (will be replaced with real UI in Stage 3)
RUN mkdir -p ui/dist && echo '<!DOCTYPE html><html><body>Building...</body></html>' > ui/dist/index.html

# Build dependencies with cache mount
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release -p streamkit-server --bin skit --features "moq" && \
    # Copy compiled artifacts out of cache mount so they persist in the layer
    mkdir -p /build/target-out && \
    cp -r /build/target/release /build/target-out/

# Stage 2: Build UI
FROM oven/bun:1.3.5-alpine AS ui-builder

WORKDIR /build/ui

# Install UI dependencies
COPY ui/package.json ui/bun.lock* ./
RUN --mount=type=cache,target=/root/.bun/install/cache \
    bun install --frozen-lockfile

# Copy UI source and build
COPY ui/ ./
RUN bun run build

# Stage 3: Build final server binary with UI embedded
FROM rust:1.92-slim-bookworm AS rust-builder

WORKDIR /build

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    g++ \
    cmake \
    libopus-dev \
    libclang-dev \
    clang \
    curl \
    git \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY apps ./apps
COPY crates ./crates
COPY sdks ./sdks
COPY wit ./wit

# Copy built UI from stage 2
COPY --from=ui-builder /build/ui/dist ./ui/dist

# Build server with cache mount
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    --mount=type=bind,from=rust-deps,source=/build/target-out/release,target=/build/target-init \
    bash -c '\
      # Copy pre-built dependencies if target is empty (first build) \
      if [ ! -d "/build/target/release/deps" ]; then \
        echo "Initializing target from cache..."; \
        cp -r /build/target-init/* /build/target/release/ || true; \
      fi; \
      # Remove server binary to force rebuild with new UI \
      rm -rf /build/target/release/skit \
        /build/target/release/skit.d \
        /build/target/release/deps/streamkit_server-* \
        /build/target/release/.fingerprint/streamkit-server-*; \
      # Build only the server binary \
      cargo build --locked --release --features "moq" --bin skit; \
      # Copy final binary out of cache mount \
      mkdir -p /build/bin && cp /build/target/release/skit /build/bin/skit \
    '

# Runtime stage - minimal image with server + samples
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libopus0 \
    libgomp1 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create app user
RUN useradd -m -u 1000 -s /bin/bash app

# Copy binary from rust builder
COPY --from=rust-builder /build/bin/skit /usr/local/bin/skit

# Copy sample pipelines
COPY --chown=app:app samples/pipelines /opt/streamkit/samples/pipelines

# Copy small bundled audio samples (Opus/Ogg only) for quickstart/tests
COPY --chown=app:app samples/audio/system/*.ogg samples/audio/system/*.ogg.license /opt/streamkit/samples/audio/system/
COPY --chown=app:app samples/audio/system/*.opus samples/audio/system/*.opus.license /opt/streamkit/samples/audio/system/

# Copy Docker configuration
COPY --chown=app:app docker-skit.toml /opt/streamkit/skit.toml

# Create directories for external mounts (models, plugins)
# Users should mount models and plugins at runtime:
#   -v /path/to/models:/opt/streamkit/models:ro
#   -v /path/to/plugins:/opt/streamkit/plugins:ro
RUN mkdir -p /opt/streamkit/models /opt/streamkit/plugins /opt/streamkit/.plugins /opt/streamkit/logs && \
    chown -R app:app /opt/streamkit

WORKDIR /opt/streamkit
USER app

# Expose HTTP and UDP ports
EXPOSE 4545/tcp
EXPOSE 4545/udp

# OCI image labels
LABEL org.opencontainers.image.title="StreamKit"
LABEL org.opencontainers.image.description="High-performance real-time media processing engine (slim image - mount models/plugins externally; includes sample pipelines/audio)"
LABEL org.opencontainers.image.source="https://github.com/streamer45/streamkit"
LABEL org.opencontainers.image.licenses="MPL-2.0"
LABEL org.opencontainers.image.vendor="StreamKit Contributors"

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:4545/healthz || exit 1

# Default command
CMD ["skit", "serve"]
