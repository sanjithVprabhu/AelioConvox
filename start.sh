#!/usr/bin/env bash
# Boot Aelio locally: install deps, build Rust + TS workspaces, start server + example SDK.
# Equivalent to: pnpm start  →  node scripts/start.mjs
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

usage() {
  cat <<'EOF'
Usage: ./start.sh [options]

Boot the full local Aelio stack:
  • authoritative Rust runtime (aelio-server + Aelio DB)
  • TypeScript channel/model edge (@aelio/server)
  • bundled ShopCo example SDK (unless skipped)

Requires: Node.js >= 20, pnpm 9, Rust/cargo
Optional: copy .env.example → .env for API keys and provider config

Options:
  -h, --help       Show this help
  --clean-data     Delete data/aelio-os/ before boot (fixes stale WAL / tenant tool errors)

Environment (see .env.example):
  AELIO_PORT              HTTP/WS port (default 3010)
  AELIO_CONFIG            Config YAML (default config.yaml; use config.openai.yaml for OpenAI)
  AELIO_SKIP_EXAMPLE_SDK  Set to 1 when using an external SDK (e.g. aelio-test-3)
  AELIO_HARNESS_MODE      agent_loop | legacy | sol | auto (default agent_loop)

After start:
  Demo:   http://localhost:3010/demo.html
  Health: http://localhost:3010/health
  Ready:  http://localhost:3010/ready

EOF
}

CLEAN_DATA=0
START_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --clean-data)
      CLEAN_DATA=1
      shift
      ;;
    *)
      START_ARGS+=("$1")
      shift
      ;;
    esac
done

if ! command -v node >/dev/null 2>&1; then
  echo "error: Node.js >= 20 is required" >&2
  exit 1
fi

NODE_MAJOR="$(node -e "process.stdout.write(process.versions.node.split('.')[0])")"
if (( NODE_MAJOR < 20 )); then
  echo "error: Node.js >= 20 is required (found $(node -v))" >&2
  exit 1
fi

if ! command -v pnpm >/dev/null 2>&1; then
  echo "error: pnpm is required. Run: corepack enable && corepack prepare pnpm@9 --activate" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: Rust/cargo is required to build aelio-server (aelio-os/)" >&2
  exit 1
fi

if (( CLEAN_DATA )); then
  echo "[start] clearing local dev data at data/aelio-os/"
  rm -rf "$ROOT/data/aelio-os"
  mkdir -p "$ROOT/data/aelio-os"
fi

exec node scripts/start.mjs "${START_ARGS[@]}"
