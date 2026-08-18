# Build stage
FROM rust:1.82-bookworm AS builder

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy Cargo files
COPY backend/Cargo.toml backend/Cargo.lock ./

# Create dummy src to cache dependencies
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src

# Copy actual source
COPY backend/src ./src
COPY backend/migrations ./migrations

# Build application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

# Copy binary and assets
COPY --from=builder /app/target/release/noctalia-spotify-backend /usr/local/bin/
COPY backend/systemd/noctalia-spotify-backend.service /etc/systemd/user/
COPY backend/scripts/build-backend.sh /usr/local/bin/

# Create non-root user
RUN useradd -m -u 1000 appuser
USER appuser

ENV RUST_LOG=info
ENV XDG_RUNTIME_DIR=/run/user/1000

# Expose OAuth port
EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD nc -z localhost 8000 || exit 1

ENTRYPOINT ["noctalia-spotify-backend"]