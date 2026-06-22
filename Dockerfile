FROM rust:1.96-bookworm AS builder

# Build offline against the committed .sqlx query caches, so no database is needed.
ENV SQLX_OFFLINE=true
WORKDIR /app
COPY . .
RUN cargo build --release -p arbiter-node

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
COPY --from=builder /app/target/release/arbiter-node /usr/local/bin/arbiter-node

# Node identity (UUID + crypto keys) is persisted here.
VOLUME ["/data"]
EXPOSE 8080

# One binary, one image. A container picks its roles via env, e.g.
# ARBITER_ROLES_API=false ARBITER_ROLES_SCHEDULER=false (a worker-only node),
# or leave them unset for an all-in-one node (all roles on).
CMD ["arbiter-node"]
