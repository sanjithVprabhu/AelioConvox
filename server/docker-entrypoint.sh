#!/bin/bash
# Starts the authoritative Rust Aelio OS/database, then the TypeScript host/transport edge.
set -euo pipefail

if [[ -z "${AELIO_INTERNAL_TOKEN:-}" || "${#AELIO_INTERNAL_TOKEN}" -lt 32 ]]; then
  echo "[aelio] AELIO_INTERNAL_TOKEN is required and must contain at least 32 characters" >&2
  exit 1
fi
export AELIO_RUNTIME_TOKENS="${AELIO_RUNTIME_TOKENS:-$AELIO_INTERNAL_TOKEN}"
export AELIO_RUNTIME_TOKEN="${AELIO_RUNTIME_TOKEN:-$AELIO_INTERNAL_TOKEN}"
export AELIO_EVENT_KEY_SECRET="${AELIO_EVENT_KEY_SECRET:-$AELIO_INTERNAL_TOKEN}"
export AELIO_HOST_TOKEN="${AELIO_HOST_TOKEN:-$AELIO_INTERNAL_TOKEN}"
export AELIO_LLM_GATEWAY_TOKEN="${AELIO_LLM_GATEWAY_TOKEN:-$AELIO_HOST_TOKEN}"
export AELIO_LLM_GATEWAY_URL="${AELIO_LLM_GATEWAY_URL:-http://127.0.0.1:3000/internal/aelio/llm/complete}"
export AELIO_LLM_EMBED_URL="${AELIO_LLM_EMBED_URL:-http://127.0.0.1:3000/internal/aelio/llm/embed}"
export AELIO_TENANT_ID="${AELIO_TENANT_ID:-aelio-docker}"
export DB_API_KEY="${DB_API_KEY:-$AELIO_INTERNAL_TOKEN}"

mkdir -p "${AELIO_DATA_DIR:-/data}"

aelio-server &
RUST_PID=$!
cleanup() {
  if kill -0 "$RUST_PID" 2>/dev/null; then
    kill "$RUST_PID" 2>/dev/null || true
    wait "$RUST_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

ready=0
for _ in $(seq 1 90); do
  if curl -fsS "${AELIO_RUST_RUNTIME_URL:-http://127.0.0.1:8090}/readyz" >/dev/null 2>&1; then
    ready=1
    break
  fi
  if ! kill -0 "$RUST_PID" 2>/dev/null; then
    echo "[aelio] Rust runtime exited before becoming ready" >&2
    exit 1
  fi
  sleep 0.5
done
if [[ "$ready" -ne 1 ]]; then
  echo "[aelio] timed out waiting for the Rust runtime" >&2
  exit 1
fi

exec node /app/dist/main.js
