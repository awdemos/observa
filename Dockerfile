# syntax=docker/dockerfile:1

# Multi-stage Dockerfile for Observa using Docker Hub images.
# Builder compiles a release binary; runtime is a minimal Debian base.
#
# Default registry: docker.io. Override with:
#   docker build --build-arg REGISTRY=my-mirror.example.com ...
ARG REGISTRY=docker.io

# -----------------------------------------------------------------------------
# Builder stage: compile the release binary
# -----------------------------------------------------------------------------
FROM ${REGISTRY}/library/rust:1-bookworm AS builder
WORKDIR /src

# Copy the workspace manifest and all crate sources, then build the release
# binary.
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN cargo build --release -p observa-cli --locked

# -----------------------------------------------------------------------------
# Runtime stage: minimal Debian base
# -----------------------------------------------------------------------------
FROM ${REGISTRY}/library/debian:bookworm-slim

# Only runtime libraries required by SQLite and HTTPS outbound calls.
RUN apt-get update \
    && apt-get install -y --no-install-recommends sqlite3 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary and runtime assets.
COPY --from=builder /src/target/release/observa /usr/local/bin/observa
COPY templates/ ./templates/
COPY assets/ ./assets/

# Runtime data directory for SQLite and optional log files.
RUN groupadd -r nonroot && useradd -r -g nonroot nonroot \
    && mkdir -p /data/logs \
    && chown -R nonroot:nonroot /data /app

# Run as an unprivileged user.
USER nonroot

VOLUME ["/data"]

# Bind to localhost by default. Override to 0.0.0.0 only when a reverse proxy
# provides authentication/TLS.
ENV OBSERVA_BIND=127.0.0.1:3000 \
    OBSERVA_DATABASE_URL=sqlite:///data/observa.db \
    RUST_LOG=info

EXPOSE 3000

ENTRYPOINT ["observa"]
