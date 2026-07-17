#!/usr/bin/env bash
set -euo pipefail

server_port="${1:-8787}"
repo_root="$(cd "$(dirname "$0")/../../.." && pwd)"
pg_root="$(mktemp -d /tmp/tempestmiku-web-pg.XXXXXX)"
pg_data="$pg_root/data"
pg_socket="$pg_root/socket"
pg_pid=""
mkdir -p "$pg_socket"

cleanup() {
  if [[ -n "$pg_pid" ]]; then
    kill "$pg_pid" >/dev/null 2>&1 || true
    wait "$pg_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$pg_root"
}
trap cleanup EXIT INT TERM

initdb -D "$pg_data" -A trust -U postgres >/dev/null
postgres -D "$pg_data" -k "$pg_socket" -h '' >"$pg_root/postgres.log" 2>&1 &
pg_pid="$!"
for _ in {1..100}; do
  if pg_isready -h "$pg_socket" -U postgres -d postgres >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$pg_pid" >/dev/null 2>&1; then
    cat "$pg_root/postgres.log" >&2
    exit 1
  fi
  sleep 0.1
done
if ! pg_isready -h "$pg_socket" -U postgres -d postgres >/dev/null 2>&1; then
  cat "$pg_root/postgres.log" >&2
  exit 1
fi

cd "$repo_root"
OPENAI_API_KEY= \
OPENAI_BASE_URL= \
TM_OMP_ACP_ENABLED=0 \
TM_SERVER_ROLE=all \
TM_SERVER_ADDR="127.0.0.1:$server_port" \
TM_DATABASE_URL="host=$pg_socket user=postgres dbname=postgres" \
cargo run -p tm-server
