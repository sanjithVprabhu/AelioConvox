#!/usr/bin/env bash
# Stop stale local Aelio test/smoke containers (reversible with docker start).
# Does not remove volumes or images. Run from repo root:
#   bash scripts/docker-cleanup-stale.sh
set -euo pipefail

NAMES=(
  aelio-sunjet-1
  aelio-sunjet-daemon-1
  aelio-harness-smoke
  aelio-restructure-smoke
  aelio-server-publish-test
  aelio-pg-test
)

running=()
for name in "${NAMES[@]}"; do
  if docker ps -q -f "name=^${name}$" | grep -q .; then
    running+=("$name")
  fi
done

if [ "${#running[@]}" -eq 0 ]; then
  echo "No stale aelio test containers running."
  exit 0
fi

echo "Stopping: ${running[*]}"
docker stop "${running[@]}"
echo "Done. Restart any with: docker start <name>"