# =============================================================================
# Gita - AI-native Git Server
# Single-stage Dockerfile using locally built release binary
# Uses locally available base image to avoid network pulls
# =============================================================================

# Use locally available Debian slim image
FROM docker.io/library/debian:bookworm-slim

# Install runtime dependencies:
# - git: git CLI for git protocol backend
# - libssl3: OpenSSL for reqwest/rustls
# - curl: healthchecks
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    git \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN groupadd -r gita && useradd -r -g gita -d /data -s /sbin/nologin gita

WORKDIR /app

# Copy locally built release binary
COPY target/release/gita-server /app/gita-server

# Copy built frontend next to the binary. The server resolves it at runtime
# via `find_web_dist()`: <exe_dir>/web/dist — no host-specific paths needed.
COPY web/dist /app/web/dist

# Copy config example as template
COPY config/config.example.toml /app/config.example.toml

# Copy entrypoint script
COPY docker-entrypoint.sh /app/docker-entrypoint.sh
RUN chmod +x /app/docker-entrypoint.sh

# Create data directory with correct ownership
RUN mkdir -p /data/repos /app/config && chown -R gita:gita /data /app

# Keep the working directory at /app; the binary resolves the frontend
# relative to its own location, so no host-specific path is required.
WORKDIR /app

# Switch to non-root user
USER gita

# Expose HTTP port (Git Smart HTTP + REST API + Web UI)
EXPOSE 3000

# Health check endpoint
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://127.0.0.1:3000/api/health || exit 1

# Environment variables with defaults (override at runtime)
ENV GITA_SERVER_HOST=0.0.0.0 \
    GITA_SERVER_PORT=3000 \
    GITA_SERVER_DATA_DIR=/data \
    GITA_SERVER_DATABASE_URL=sqlite:///data/gita.db \
    GITA_SERVER_JWT_SECRET=dev-secret-change-me

# Entry point: generate config from env, then run server
ENTRYPOINT ["/app/docker-entrypoint.sh"]
