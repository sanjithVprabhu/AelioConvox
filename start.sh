#!/usr/bin/env bash
# Local dev entrypoint — installs deps, builds Sunjet ll-server if needed,
# then boots Sunjet + Aelio server + example SDK (see scripts/start.mjs).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

node_major="$(node -p "process.versions.node.split('.')[0]")"
if [ "$node_major" -lt 20 ]; then
  echo "[start] error: Node.js >= 20 required (found $(node -v))" >&2
  exit 1
fi

if ! command -v pnpm >/dev/null 2>&1; then
  if command -v corepack >/dev/null 2>&1; then
    corepack enable >/dev/null 2>&1 || true
    corepack prepare pnpm@9.15.4 --activate >/dev/null 2>&1 || true
  fi
fi

if ! command -v pnpm >/dev/null 2>&1; then
  echo "[start] error: pnpm is required. Run: corepack enable && corepack prepare pnpm@9 --activate" >&2
  exit 1
fi

if [ ! -f Sunjet/Astrolobe/Cargo.toml ] && [ -d .git ]; then
  echo "[start] setup initializing Sunjet submodule…"
  git submodule update --init --recursive Sunjet/Astrolobe
fi

exec node scripts/start.mjs "$@"
