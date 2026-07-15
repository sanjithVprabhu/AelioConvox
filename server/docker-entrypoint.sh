#!/bin/bash
# Starts Sunjet (ll-server) + Aelio in one container.
set -euo pipefail

export LL_DATA_DIR="${LL_DATA_DIR:-/data/sunjet}"
export LL_BIND="${LL_BIND:-127.0.0.1:8080}"

# Shared secret between Aelio ↔ ll-server (default is fine for single-node docker).
if [[ -z "${LL_API_KEYS:-}" ]]; then
  export LL_API_KEYS="${SUNJET_API_KEY:-aelio-local}"
fi
export SUNJET_API_KEY="${SUNJET_API_KEY:-${LL_API_KEYS%%,*}}"

# Map Aelio segment env → LL_* aliases ll-server also reads.
if [[ -n "${AELIO_SUNJET_SEGMENT_BACKEND:-}" && -z "${LL_SEGMENT_BACKEND:-}" ]]; then
  export LL_SEGMENT_BACKEND="$AELIO_SUNJET_SEGMENT_BACKEND"
fi
for pair in \
  AELIO_SUNJET_S3_BUCKET:LL_S3_BUCKET \
  AELIO_SUNJET_S3_REGION:LL_S3_REGION \
  AELIO_SUNJET_S3_ENDPOINT:LL_S3_ENDPOINT \
  AELIO_SUNJET_S3_ACCESS_KEY_ID:LL_S3_ACCESS_KEY_ID \
  AELIO_SUNJET_S3_SECRET_ACCESS_KEY:LL_S3_SECRET_ACCESS_KEY \
  AELIO_SUNJET_S3_ALLOW_HTTP:LL_S3_ALLOW_HTTP \
  AELIO_SUNJET_S3_PATH_STYLE:LL_S3_PATH_STYLE \
  AELIO_SUNJET_SEGMENT_PREFIX:LL_SEGMENT_PREFIX
do
  src="${pair%%:*}"
  dst="${pair##*:}"
  src_val="${!src:-}"
  dst_val="${!dst:-}"
  if [[ -n "$src_val" && -z "$dst_val" ]]; then
    export "$dst=$src_val"
  fi
done

mkdir -p /data "$LL_DATA_DIR"

SUNJET_PID=""
cleanup() {
  if [[ -n "$SUNJET_PID" ]] && kill -0 "$SUNJET_PID" 2>/dev/null; then
    kill "$SUNJET_PID" 2>/dev/null || true
    wait "$SUNJET_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

# AELIO_SUNJET_ENABLED=0 → SQLite-only (skip Astrolobe).
# AELIO_SKIP_EMBEDDED_SUNJET=1 → use an external ll-server (compose split) without
# starting a second copy inside this container.
case "${AELIO_SKIP_EMBEDDED_SUNJET:-0}" in
  1|true|TRUE|yes|YES|on|ON)
    export AELIO_SUNJET_ENABLED="${AELIO_SUNJET_ENABLED:-1}"
    echo "[aelio] embedded Sunjet skipped — using ${AELIO_SUNJET_URL:-external}"
    ;;
  *)
    case "${AELIO_SUNJET_ENABLED:-1}" in
      0|false|FALSE|no|NO|off|OFF)
        export AELIO_SUNJET_ENABLED=0
        echo "[aelio] Sunjet disabled (AELIO_SUNJET_ENABLED=0) — SQLite only"
        ;;
      *)
        export AELIO_SUNJET_ENABLED=1
        export AELIO_SUNJET_URL="${AELIO_SUNJET_URL:-http://127.0.0.1:8080}"
        echo "[aelio] starting ll-server on ${LL_BIND} (data=${LL_DATA_DIR})…"
        ll-server &
        SUNJET_PID=$!

        ready=0
        for _ in $(seq 1 90); do
          if curl -fsS "http://127.0.0.1:${LL_BIND##*:}/v1/health" >/dev/null 2>&1; then
            ready=1
            break
          fi
          if ! kill -0 "$SUNJET_PID" 2>/dev/null; then
            echo "[aelio] ll-server exited before becoming healthy" >&2
            exit 1
          fi
          sleep 0.5
        done
        if [[ "$ready" -ne 1 ]]; then
          echo "[aelio] timed out waiting for ll-server /v1/health" >&2
          exit 1
        fi
        echo "[aelio] ll-server ready"
        ;;
    esac
    ;;
esac

cd /app
exec node dist/main.js
