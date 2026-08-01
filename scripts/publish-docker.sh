#!/usr/bin/env bash
# Build and push the all-in-one Aelio image (server + AelioDb/aelio-server).
# Also refreshes the standalone aelio-aelioDb image for advanced split deploys.
#
# Usage:
#   docker login
#   VERSION=0.1.2 ./scripts/publish-docker.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

IMAGE_NS="${DOCKER_NAMESPACE:-sanjithvprabhu}"
VERSION="${VERSION:-0.1.2}"
SERVER_LOCAL="aelio/server:latest"
SERVER_REMOTE="${IMAGE_NS}/aelio-server"
AELIO_DB_LOCAL="aelio/aelioDb:latest"
AELIO_DB_REMOTE="${IMAGE_NS}/aelio-aelioDb"

echo "==> Building all-in-one ${SERVER_LOCAL} (Aelio + AelioDb)..."
docker build -f server/Dockerfile \
  --build-arg "AELIO_VERSION=${VERSION}" \
  -t "${SERVER_LOCAL}" \
  -t "${SERVER_REMOTE}:latest" \
  -t "${SERVER_REMOTE}:${VERSION}" \
  .

echo "==> Building standalone ${AELIO_DB_LOCAL} (optional split deploy)..."
docker build -f aelio-os/Dockerfile \
  -t "${AELIO_DB_LOCAL}" \
  -t "${AELIO_DB_REMOTE}:latest" \
  -t "${AELIO_DB_REMOTE}:${VERSION}" \
  aelio-os

echo "==> Pushing ${SERVER_REMOTE}:latest and :${VERSION} ..."
docker push "${SERVER_REMOTE}:latest"
docker push "${SERVER_REMOTE}:${VERSION}"

echo "==> Pushing ${AELIO_DB_REMOTE}:latest and :${VERSION} ..."
docker push "${AELIO_DB_REMOTE}:latest"
docker push "${AELIO_DB_REMOTE}:${VERSION}"

echo ""
echo "Done. All-in-one (recommended):"
echo "  docker pull ${SERVER_REMOTE}:latest"
echo "  docker run -d -p 3010:3000 -v aelio-data:/data \\"
echo "    -e AELIO_SDK_SECRET=change-me \\"
echo "    -e AELIO_LLM_PROVIDER=openai -e OPENAI_API_KEY=sk-... \\"
echo "    ${SERVER_REMOTE}:latest"
echo ""
echo "Cloud .vss: add -e AELIO_DB_SEGMENT_BACKEND=s3 -e AELIO_DB_S3_*"
echo "SQLite only: add -e AELIO_DB_ENABLED=0"
