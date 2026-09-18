# Multi-stage Dockerfile for Agro
# Stage 1: Build the React dashboard
FROM node:20-alpine AS dashboard-builder
WORKDIR /app/dashboard
COPY dashboard/package*.json ./
RUN npm ci --ignore-scripts
COPY dashboard/ ./
RUN npm run build

# Stage 2: Compile the Rust server binary
FROM rust:1.80-bookworm AS rust-builder
WORKDIR /app
# Pre-create directory for rust-embed
COPY --from=dashboard-builder /app/dashboard/dist ./dashboard/dist
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
RUN cargo build --release --locked

# Stage 3: Minimal runtime container
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -r agro --gid 1000 \
    && useradd -r -g agro --uid 1000 -d /opt/agro -s /sbin/nologin agro

WORKDIR /opt/agro
COPY --from=rust-builder /app/target/release/agro ./agro

RUN mkdir -p /opt/agro/data /opt/agro/spool /srv/music && \
    chown -R agro:agro /opt/agro /srv/music

USER agro

ENV PORT=8700 \
    AGRO_LIBRARY_ROOT=/srv/music \
    AGRO_SPOOL_ROOT=/opt/agro/spool

WORKDIR /opt/agro/data
EXPOSE 8700

ENTRYPOINT ["/opt/agro/agro"]
