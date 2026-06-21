FROM rust:1.96-bookworm AS builder

# Build offline against the committed .sqlx query caches, so no database is needed.
ENV SQLX_OFFLINE=true
WORKDIR /app
COPY . .
RUN cargo build --release -p arbiter-api -p arbiter-node

FROM debian:bookworm-slim

ARG VCS_REF=unknown

LABEL org.opencontainers.image.title="Arbiter" \
      org.opencontainers.image.description="Distributed job scheduler (Rust + Axum)" \
      org.opencontainers.image.source="https://github.com/Axmouth/arbiter" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.licenses="MIT"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/arbiter-api /usr/local/bin/arbiter-api
COPY --from=builder /app/target/release/arbiter-node /usr/local/bin/arbiter-node

# Worker identity (a UUID file) is persisted here.
VOLUME ["/data"]
EXPOSE 8080

# The API server is the default entrypoint; override the command with
# `arbiter-node` to run a scheduler/worker node from the same image.
CMD ["arbiter-api"]
