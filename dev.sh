#!/usr/bin/env bash
#
# One-command local dev: start Postgres, build the web UI, run an all-in-one node.
# Then open http://localhost:8080 and log in with admin / admin.
#
#   ./dev.sh             # build the UI and run the node
#   ./dev.sh --skip-ui   # reuse the existing ui_dist (faster restarts)
#
# For UI hot reload instead, run `cd web-ui && npm run dev` (serves :5173 against
# this API) in a second terminal.

set -euo pipefail

cd "$(dirname "$0")"

SKIP_UI=0
[ "${1:-}" = "--skip-ui" ] && SKIP_UI=1

COMPOSE_FILE=docker/docker-compose.yml
PG_CONTAINER=arbiter_pg
# Override if 8080 is taken: `ARBITER_API_PORT=8090 ./dev.sh`
PORT="${ARBITER_API_PORT:-8080}"
export ARBITER_API_PORT="$PORT"

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$1"; }

# Pick the docker compose invocation that exists.
if docker compose version >/dev/null 2>&1; then
  COMPOSE="docker compose"
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE="docker-compose"
else
  echo "error: need Docker with 'docker compose' (or docker-compose) installed." >&2
  exit 1
fi

step "Starting Postgres (compose service 'postgres')"
$COMPOSE -f "$COMPOSE_FILE" up -d postgres

# Force a TCP check (-h/-p): on first run the entrypoint runs init scripts against a
# socket-only temp server, so a local-socket pg_isready would report ready while the
# real TCP server (what the node connects to) is still down.
step "Waiting for Postgres to accept TCP connections"
pg_ready() {
  docker exec "$PG_CONTAINER" pg_isready -h 127.0.0.1 -p 5432 -U arbiter -d arbiter \
    >/dev/null 2>&1
}
for _ in $(seq 1 120); do
  if pg_ready; then
    echo "Postgres is ready."
    break
  fi
  sleep 0.5
done
if ! pg_ready; then
  echo "error: Postgres did not become ready in time." >&2
  exit 1
fi

# Local config (gitignored). The example already points at the compose Postgres
# (localhost:2345) with dev credentials, so just copy it on first run.
if [ ! -f config/arbiter.toml ]; then
  step "Creating config/arbiter.toml from the example"
  cp config/arbiter.example.toml config/arbiter.toml
fi

if [ "$SKIP_UI" -eq 0 ]; then
  step "Building the web UI into ui_dist"
  pushd web-ui >/dev/null
  if [ ! -d node_modules ]; then
    npm install
  fi
  npm run build
  popd >/dev/null
else
  step "Skipping UI build (--skip-ui); serving the existing ui_dist"
fi

mkdir -p .dev/data

step "Starting the node (api + scheduler + worker)"
echo "    UI + API:  http://localhost:$PORT"
echo "    Swagger:   http://localhost:$PORT/swagger-ui"
echo "    Login:     admin / admin"
echo

# Node identity lives under .dev so it needs no root-owned /data; offline build uses
# the committed sqlx caches.
export SQLX_OFFLINE=true
export ARBITER_DATA_DIR=./.dev/data
export ARBITER_NODE_IDENTITY=./.dev/node_identity.json

exec cargo run -p arbiter-node
