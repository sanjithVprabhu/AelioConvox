#!/usr/bin/env bash
# Session A.4 — exactly one §4.3 canonical writer path in aelio-sol.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOL="$ROOT/aelio-os/crates/aelio-sol/src"

if grep -Rq 'fn write_task_graph' "$SOL"; then
  echo "FAIL: independent TaskGraph canonical writer still present in task_graph.rs"
  exit 1
fi

writers=$(grep -Rl 'fn write_value(' "$SOL" || true)
count=$(printf '%s\n' "$writers" | sed '/^$/d' | wc -l | tr -d ' ')
if [[ "$count" -ne 1 ]]; then
  echo "FAIL: expected exactly one write_value in aelio-sol, found: ${writers:-none}"
  exit 1
fi
if [[ "$writers" != *"canonical.rs" ]]; then
  echo "FAIL: write_value must live in canonical.rs, found in: $writers"
  exit 1
fi

echo "OK: single canonical writer (canonical.rs)"
