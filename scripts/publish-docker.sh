#!/usr/bin/env bash
# Build and push the all-in-one Aelio image (server + Sunjet/ll-server).
# Also refreshes the standalone aelio-sunjet image for advanced split deploys.
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
SUNJET_LOCAL="aelio/sunjet:latest"
SUNJET_REMOTE="${IMAGE_NS}/aelio-sunjet"

echo "==> Building all-in-one ${SERVER_LOCAL} (Aelio + Sunjet)..."
docker build -f server/Dockerfile \
  --build-arg "AELIO_VERSION=${VERSION}" \
  -t "${SERVER_LOCAL}" \
  -t "${SERVER_REMOTE}:latest" \
  -t "${SERVER_REMOTE}:${VERSION}" \
  .

echo "==> Building standalone ${SUNJET_LOCAL} (optional split deploy)..."
docker build -f Sunjet/Astrolobe/Dockerfile \
  -t "${SUNJET_LOCAL}" \
  -t "${SUNJET_REMOTE}:latest" \
  -t "${SUNJET_REMOTE}:${VERSION}" \
  Sunjet/Astrolobe

echo "==> Pushing ${SERVER_REMOTE}:latest and :${VERSION} ..."
docker push "${SERVER_REMOTE}:latest"
docker push "${SERVER_REMOTE}:${VERSION}"

echo "==> Pushing ${SUNJET_REMOTE}:latest and :${VERSION} ..."
docker push "${SUNJET_REMOTE}:latest"
docker push "${SUNJET_REMOTE}:${VERSION}"

echo ""
echo "Done. All-in-one (recommended):"
echo "  docker pull ${SERVER_REMOTE}:latest"
echo "  docker run -d -p 3010:3000 -v aelio-data:/data \\"
echo "    -e AELIO_SDK_SECRET=change-me \\"
echo "    -e AELIO_LLM_PROVIDER=openai -e OPENAI_API_KEY=sk-... \\"
echo "    ${SERVER_REMOTE}:latest"
echo ""
echo "Cloud .vss: add -e AELIO_SUNJET_SEGMENT_BACKEND=s3 -e AELIO_SUNJET_S3_*"
echo "SQLite only: add -e AELIO_SUNJET_ENABLED=0"
