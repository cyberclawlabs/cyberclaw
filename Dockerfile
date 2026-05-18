# ============================================
# CyberClaw Server - production-grade multi-stage Dockerfile
# ============================================
#
# Stages:
#   1. builder     — Rust toolchain compiles `cyberclaw-server` (release).
#   2. web-builder — Node Alpine compiles JSX → web/dist/*.js via babel.
#   3. runtime     — Debian slim, non-root, with binary + SPA assets only.
#
# Notes:
#   - The previous "deps cache" stage that synthesised stub `src/lib.rs`
#     files for layer caching was brittle (broke when crates declared
#     `[[bench]]` blocks pointing at non-existent paths). It is gone.
#     If you need incremental builds, mount a cargo registry cache via
#     `--mount=type=cache,target=/usr/local/cargo/registry` (BuildKit only).
#
# Usage:
#   podman build -t cyberclaw-server:dev .
#   ./scripts/deploy/staging-podman.sh up

# ============================================
# Stage 1: Rust build
# ============================================
FROM rust:1-slim-bookworm AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    sqlite3 \
    libsqlite3-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY apps/ apps/
COPY crates/ crates/

RUN cargo build --release -p cyberclaw-server && \
    strip target/release/cyberclaw-server

# ============================================
# Stage 2: SPA precompile (JSX → IIFE-wrapped JS)
# ============================================
# `cyberclaw.html` no longer carries `<script type="text/babel">` — it loads
# precompiled bundles from `/admin/dist/*.js` (see `babel.config.js` for
# the IIFE-wrap + window-expose plugin). This stage produces those files.
FROM node:20-alpine AS web-builder

WORKDIR /app

COPY package.json package-lock.json babel.config.js ./
RUN npm ci --no-audit --no-fund

COPY web/ web/
RUN npm run build:web

# ============================================
# Stage 3: Minimal runtime
# ============================================
FROM debian:bookworm-slim AS runtime

# - ca-certificates: TLS verification
# - libssl3:        OpenSSL runtime
# - sqlite3:        SQLite runtime (CYBERCLAW_MEMORY_DB persistence)
# - curl:           HEALTHCHECK probe
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    sqlite3 \
    curl \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

RUN groupadd -g 1000 cyberclaw && \
    useradd -m -u 1000 -g cyberclaw -s /bin/false cyberclaw

WORKDIR /app

COPY --from=builder /app/target/release/cyberclaw-server /app/cyberclaw-server
# Admin SPA assets — `serve_admin_html` and `serve_admin_dist` read from
# `${CYBERCLAW_WEB_ROOT}` (default `/app/web`).
COPY --from=web-builder /app/web /app/web
# Ecosystem packages — `bootstrap_registry_from_ecosystem` is load-bearing
# (server fails to start with 0 packages). Resolved path defaults to
# `/app/apps/cyberclaw-server/../../ecosystem` from build-time CARGO_MANIFEST_DIR;
# placing the bundle at `/app/ecosystem` keeps that path valid, and the
# runtime env `CYBERCLAW_ECOSYSTEM_DIR` overrides it explicitly anyway.
COPY ecosystem /app/ecosystem
# Server config — `/api/v1/settings/config` reads the TOML body from
# `CYBERCLAW_CONFIG_PATH`. Without this file the endpoint 404s and the
# admin Operator-Settings page errors. The compile-time fallback path
# (CARGO_MANIFEST_DIR/config.toml) doesn't exist in the runtime image.
COPY apps/cyberclaw-server/config.toml /app/config.toml
# Entrypoint — seeds demo users when SEED_DEMO_USERS=1; pass-through otherwise.
COPY scripts/deploy/entrypoint.sh /app/entrypoint.sh

RUN chown cyberclaw:cyberclaw /app/cyberclaw-server /app/entrypoint.sh /app/config.toml && \
    chown -R cyberclaw:cyberclaw /app/web /app/ecosystem && \
    chmod 500 /app/cyberclaw-server && \
    chmod 550 /app/entrypoint.sh && \
    chmod 644 /app/config.toml

# SQLite memory DB lives under a writable, persistable mount.
RUN mkdir -p /var/lib/cyberclaw && chown cyberclaw:cyberclaw /var/lib/cyberclaw

USER cyberclaw

EXPOSE 3000

ENV CYBERCLAW_ADDR=0.0.0.0:3000 \
    ENVIRONMENT=production \
    CYBERCLAW_MEMORY_DB=/var/lib/cyberclaw/memory.db \
    CYBERCLAW_WEB_ROOT=/app/web \
    CYBERCLAW_ECOSYSTEM_DIR=/app/ecosystem \
    CYBERCLAW_CONFIG_PATH=/app/config.toml

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

LABEL org.opencontainers.image.title="CyberClaw Server" \
      org.opencontainers.image.description="CyberClaw controlled agent platform HTTP server" \
      org.opencontainers.image.source="https://github.com/cyberclawlabs/cyberclaw" \
      org.opencontainers.image.licenses="Apache-2.0"

ENTRYPOINT ["/app/entrypoint.sh"]
CMD ["/app/cyberclaw-server"]
