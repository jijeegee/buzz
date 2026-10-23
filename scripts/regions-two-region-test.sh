#!/usr/bin/env bash
# Two-region routing test (global regions S3).
#
# Starts docker-compose.regions.yml (Postgres and Redis per region, shared
# MinIO), runs two relays from this checkout (kr primary on :3041, jp on
# :3042) and runs crates/kuzz-test-client/tests/e2e_regions.rs. Everything is
# torn down on exit.
#
# Both regions serve one service: RELAY_URL is kr's address and each region's
# own address is a service alias. jp's global tables are a copy of kr's (S5):
# kr publishes them, jp's database subscribes, and jp is added as a region in
# kr, the way an operator adds a region.
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
# One service: the same relay key and community id in both regions.
RELAY_KEY="$(openssl rand -hex 32)"
COMMUNITY_ID="$(openssl rand -hex 16 | sed -E 's/(.{8})(.{4})(.{4})(.{4})(.{12})/\1-\2-\3-\4-\5/')"
# Sign-in through the dev-only fake Google provider (S5-2 forwards it to kr).
INVITE_SECRET="$(openssl rand -hex 32)"
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
cargo build -p kuzz-relay --features dev
BIN="${TARGET}/debug/kuzz-relay"
[[ -x "${BIN}" || -x "${BIN}.exe" ]] || { log "no relay binary at ${BIN}"; exit 1; }
mkdir -p "${LOG_DIR}"

# start_relay <region> <port> <postgres port> <redis port> <health port> <metrics port> <link key>
start_relay() {
  local region=$1 port=$2 pg=$3 redis=$4 health=$5 metrics=$6 link_key=$7
  env DATABASE_URL="postgres://kuzz:kuzz_dev@127.0.0.1:${pg}/kuzz" \
    REDIS_URL="redis://127.0.0.1:${redis}" \
    RELAY_URL="${KR_URL}" \
    KUZZ_SERVICE_ALIASES="localhost:${port}" \
    KUZZ_REGION_CONNECT_URL="ws://localhost:${port}" \
    KUZZ_BIND_ADDR="127.0.0.1:${port}" \
    KUZZ_HEALTH_PORT="${health}" \
    KUZZ_METRICS_PORT="${metrics}" \
    KUZZ_AUTO_MIGRATE=true \
    KUZZ_REGION_ID="${region}" \
    KUZZ_PRIMARY_REGION=kr \
    KUZZ_REGION_LINK=on \
    KUZZ_REGION_KEY="${link_key}" \
    KUZZ_COMMUNITY_ID="${COMMUNITY_ID}" \
    KUZZ_S3_ENDPOINT=http://127.0.0.1:9491 \
    KUZZ_S3_ACCESS_KEY=kuzz_dev \
    KUZZ_S3_SECRET_KEY=kuzz_dev_secret \
    KUZZ_S3_BUCKET=kuzz-media \
    KUZZ_RELAY_PRIVATE_KEY="${RELAY_KEY}" \
    KUZZ_REQUIRE_AUTH_TOKEN=false \
    AUTH_TOKEN_ENABLED=true \
    AUTH_OIDC_FAKE=true \
    INVITE_SIGNING_SECRET="${INVITE_SECRET}" \
    KUZZ_CHAT_STATE_WRITES=on \
    KUZZ_PUBSUB_SEQ_ENVELOPE=true \
    KUZZ_GLOBAL_COPY_MAX_LAG_SECS=3 \
    "${BIN}" >"${LOG_DIR}/relay-${region}.log" 2>&1 &
  PIDS+=($!)
}

wait_up() {
  for _ in $(seq 1 180); do
    curl -s -o /dev/null "$1" && return 0
    sleep 1
  done
  log "relay $1 did not start; see ${LOG_DIR}"
  exit 1
}

# kr, the primary, migrates, registers itself and publishes the global tables.
start_relay kr 3041 5491 6491 3051 3061 "${KR_LINK_KEY}"
wait_up http://localhost:3041/
log "kr up"

# jp's relay migrates jp's database, then waits for the copy. Subscribe once
# the tables exist.
start_relay jp 3042 5492 6492 3052 3062 "${JP_LINK_KEY}"
for _ in $(seq 1 120); do
  "${COMPOSE[@]}" exec -T postgres-jp psql -qtA -U kuzz -d kuzz -c \
    "SELECT 1 FROM pg_proc WHERE proname = 'kuzz_is_global_copy'" 2>/dev/null | grep -q 1 && break
  sleep 1
done
"${COMPOSE[@]}" exec -T postgres-jp psql -q -v ON_ERROR_STOP=1 -U kuzz -d kuzz -c \
  "CREATE SUBSCRIPTION kuzz_global
     CONNECTION 'host=postgres-kr port=5432 user=kuzz password=kuzz_dev dbname=kuzz'
     PUBLICATION kuzz_global"

# Add jp in the primary with the link key jp's relay asks for.
JP_LINK_PUBKEY=""
for _ in $(seq 1 90); do
  JP_LINK_PUBKEY="$(sed 's/\x1b\[[0-9;]*m//g' "${LOG_DIR}/relay-jp.log" \
    | grep -oE 'link_pubkey"?[=:] ?"?[0-9a-f]{64}' | head -1 | grep -oE '[0-9a-f]{64}' || true)"
  [[ -n "${JP_LINK_PUBKEY}" ]] && break
  sleep 1
done
[[ -n "${JP_LINK_PUBKEY}" ]] || { log "jp did not ask to be added; see ${LOG_DIR}"; exit 1; }
"${COMPOSE[@]}" exec -T postgres-kr psql -q -v ON_ERROR_STOP=1 -U kuzz -d kuzz -c \
  "INSERT INTO regions (id, connect_url, is_primary, link_pubkey)
   VALUES ('jp', '${JP_URL}', false, '${JP_LINK_PUBKEY}')"
wait_up http://localhost:3042/
log "relays up"

log "running e2e_regions"
REGION_KR_URL="${KR_URL}" REGION_JP_URL="${JP_URL}" \
  REGION_JP_DATABASE_URL="postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz" \
  REGION_KR_DATABASE_URL="postgres://kuzz:kuzz_dev@127.0.0.1:5491/kuzz" \
  REGION_JP_LINK_KEY="${JP_LINK_KEY}" \
  cargo test -p kuzz-test-client --test e2e_regions -- --ignored --test-threads=1

# These stop jp's copy for a while, so they run after the others (S5-4).
log "running e2e_regions_lag"
REGION_KR_URL="${KR_URL}" REGION_JP_URL="${JP_URL}" \
  REGION_JP_DATABASE_URL="postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz" \
  REGION_KR_DATABASE_URL="postgres://kuzz:kuzz_dev@127.0.0.1:5491/kuzz" \
  cargo test -p kuzz-test-client --test e2e_regions_lag -- --ignored --test-threads=1

# The relay in jp must never write a global table itself (S5-3): its copy
# guard would refuse it.
if grep -q "is written in the primary region" "${LOG_DIR}/relay-jp.log"; then
  log "jp's relay wrote global tables itself:"
  grep "is written in the primary region" "${LOG_DIR}/relay-jp.log" | head -5
  exit 1
fi
log "passed"
