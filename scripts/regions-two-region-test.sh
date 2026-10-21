#!/usr/bin/env bash
# Two-region routing test (global regions S3).
#
# Starts docker-compose.regions.yml (Postgres and Redis per region, shared
# MinIO), runs two relays from this checkout (kr primary on :3041, jp on
# :3042), registers both regions in both databases, and runs
# crates/kuzz-test-client/tests/e2e_regions.rs. Everything is torn down on exit.
#
#   scripts/regions-two-region-test.sh            # build, run, tear down
#   KEEP_STACK=1 scripts/regions-two-region-test.sh  # leave stack and relays up
#   KUZZ_MINIO_IMAGE=minio/minio:latest scripts/...  # another MinIO image
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"
PROJECT=kuzz-regions
COMPOSE=(docker compose -p "${PROJECT}" -f docker-compose.regions.yml)
TARGET="${CARGO_TARGET_DIR:-${ROOT}/target}"
LOG_DIR="${LOG_DIR:-${TARGET}/regions-test}"
KR_URL=ws://localhost:3041
JP_URL=ws://localhost:3042
# Throwaway region link keys (S4): one per region.
KR_LINK_KEY="$(openssl rand -hex 32)"
JP_LINK_KEY="$(openssl rand -hex 32)"
PIDS=()

log() { printf '[regions] %s\n' "$*"; }

cleanup() {
  if [[ "${KEEP_STACK:-0}" == 1 ]]; then
    log "KEEP_STACK=1: relays (${PIDS[*]:-none}) and stack left running"
    return
  fi
  for pid in "${PIDS[@]:-}"; do
    [[ -n "${pid}" ]] && kill "${pid}" 2>/dev/null || true
  done
  "${COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

log "starting stack"
"${COMPOSE[@]}" up -d --wait postgres-kr postgres-jp redis-kr redis-jp minio

log "building relay"
cargo build -p kuzz-relay
BIN="${TARGET}/debug/kuzz-relay"
[[ -x "${BIN}" || -x "${BIN}.exe" ]] || { log "no relay binary at ${BIN}"; exit 1; }
mkdir -p "${LOG_DIR}"

# start_relay <region> <port> <postgres port> <redis port> <health port> <metrics port> <link key>
start_relay() {
  local region=$1 port=$2 pg=$3 redis=$4 health=$5 metrics=$6 link_key=$7
  env DATABASE_URL="postgres://kuzz:kuzz_dev@127.0.0.1:${pg}/kuzz" \
    REDIS_URL="redis://127.0.0.1:${redis}" \
    RELAY_URL="ws://localhost:${port}" \
    KUZZ_BIND_ADDR="127.0.0.1:${port}" \
    KUZZ_HEALTH_PORT="${health}" \
    KUZZ_METRICS_PORT="${metrics}" \
    KUZZ_AUTO_MIGRATE=true \
    KUZZ_REGION_ID="${region}" \
    KUZZ_PRIMARY_REGION=kr \
    KUZZ_REGION_LINK=on \
    KUZZ_REGION_KEY="${link_key}" \
    KUZZ_S3_ENDPOINT=http://127.0.0.1:9491 \
    KUZZ_S3_ACCESS_KEY=kuzz_dev \
    KUZZ_S3_SECRET_KEY=kuzz_dev_secret \
    KUZZ_S3_BUCKET=kuzz-media \
    KUZZ_RELAY_PRIVATE_KEY="$(openssl rand -hex 32)" \
    KUZZ_REQUIRE_AUTH_TOKEN=false \
    "${BIN}" >"${LOG_DIR}/relay-${region}.log" 2>&1 &
  PIDS+=($!)
}

start_relay kr 3041 5491 6491 3051 3061 "${KR_LINK_KEY}"
start_relay jp 3042 5492 6492 3052 3062 "${JP_LINK_KEY}"

for url in http://localhost:3041/ http://localhost:3042/; do
  for _ in $(seq 1 120); do
    curl -s -o /dev/null "${url}" && break
    sleep 1
  done
  curl -s -o /dev/null "${url}" || { log "relay ${url} did not start; see ${LOG_DIR}"; exit 1; }
done
log "relays up"

# Each region registers only itself; until S5 copies the region list, tell
# both databases about both regions. No room has been routed yet, so the
# relays' region caches are still empty.
for service in postgres-kr postgres-jp; do
  "${COMPOSE[@]}" exec -T "${service}" psql -q -v ON_ERROR_STOP=1 -U kuzz -d kuzz -c \
    "INSERT INTO regions (id, connect_url, is_primary) VALUES
       ('kr', '${KR_URL}', true), ('jp', '${JP_URL}', false)
     ON CONFLICT (id) DO NOTHING"
done

# Each relay recorded its own link key; give each database the other's.
link_pubkey() {
  "${COMPOSE[@]}" exec -T "postgres-$1" psql -qtA -U kuzz -d kuzz -c \
    "SELECT link_pubkey FROM regions WHERE id = '$1'"
}
KR_LINK_PUBKEY="$(link_pubkey kr | tr -d '[:space:]')"
JP_LINK_PUBKEY="$(link_pubkey jp | tr -d '[:space:]')"
[[ -n "${KR_LINK_PUBKEY}" && -n "${JP_LINK_PUBKEY}" ]] || { log "relays did not record link keys"; exit 1; }
"${COMPOSE[@]}" exec -T postgres-kr psql -q -v ON_ERROR_STOP=1 -U kuzz -d kuzz -c \
  "UPDATE regions SET link_pubkey = '${JP_LINK_PUBKEY}' WHERE id = 'jp'"
"${COMPOSE[@]}" exec -T postgres-jp psql -q -v ON_ERROR_STOP=1 -U kuzz -d kuzz -c \
  "UPDATE regions SET link_pubkey = '${KR_LINK_PUBKEY}' WHERE id = 'kr'"

log "running e2e_regions"
REGION_KR_URL="${KR_URL}" REGION_JP_URL="${JP_URL}" \
  REGION_JP_DATABASE_URL="postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz" \
  REGION_KR_DATABASE_URL="postgres://kuzz:kuzz_dev@127.0.0.1:5491/kuzz" \
  REGION_JP_LINK_KEY="${JP_LINK_KEY}" \
  cargo test -p kuzz-test-client --test e2e_regions -- --ignored --test-threads=1
log "passed"
