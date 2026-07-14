# syntax=docker/dockerfile:1

# Chainguard + cargo-chef multi-stage Dockerfile for Observa.
# Builder compiles a static-ish release binary; runtime is a minimal Wolfi base.

# -----------------------------------------------------------------------------
# Chef stage: install cargo-chef on a Chainguard Rust dev image
# -----------------------------------------------------------------------------
FROM cgr.dev/chainguard/rust:latest-dev@sha256:1bd012c6e36a4e858172fed18b8739edefa6f01887f8c72732b04ee119d513a3 AS chef
WORKDIR /src
USER root
RUN cargo install cargo-chef
ENV PATH="/root/.cargo/bin:${PATH}"

# -----------------------------------------------------------------------------
# Planner stage: generate a dependency recipe from the workspace manifest
# -----------------------------------------------------------------------------
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN cargo chef prepare --recipe-path recipe.json

# -----------------------------------------------------------------------------
# Builder stage: cook dependencies, then compile the release binary
# -----------------------------------------------------------------------------
FROM chef AS builder
COPY --from=planner /src/recipe.json recipe.json

# Build dependencies first so this layer is cached unless the recipe changes.
RUN cargo chef cook --release --recipe-path recipe.json -p observa-cli --locked

# Copy source and build the release binary.
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN cargo build --release -p observa-cli --locked

# -----------------------------------------------------------------------------
# Runtime stage: minimal Chainguard Wolfi base
# -----------------------------------------------------------------------------
FROM cgr.dev/chainguard/wolfi-base:latest@sha256:02dab76bd852a70556b5b2002195c8a5fdab77d323c433bf6642aab080489795

# Only runtime libraries required by SQLite and HTTPS outbound calls.
RUN apk add --no-cache sqlite-libs ca-certificates

WORKDIR /app

# Copy the compiled binary and runtime assets.
COPY --from=builder /src/target/release/observa /usr/local/bin/observa
COPY templates/ ./templates/
COPY assets/ ./assets/

# Runtime data directory for SQLite and optional log files.
RUN mkdir -p /data/logs && chown -R nonroot:nonroot /data /app

# Run as the unprivileged nonroot user provided by Chainguard images.
USER nonroot

VOLUME ["/data"]

# Bind to localhost by default. Override to 0.0.0.0 only when a reverse proxy
# provides authentication/TLS.
ENV OBSERVA_BIND=127.0.0.1:3000 \
    OBSERVA_DATABASE_URL=sqlite:///data/observa.db \
    RUST_LOG=info

EXPOSE 3000

ENTRYPOINT ["observa"]
