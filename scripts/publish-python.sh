#!/usr/bin/env bash
# Publish aelio-sdk to PyPI.
# Usage:
#   export TWINE_PASSWORD=pypi-...   # or PYPI_API_TOKEN
#   ./scripts/publish-python.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/sdk/python"

TOKEN="${TWINE_PASSWORD:-${PYPI_API_TOKEN:-}}"
if [[ -z "$TOKEN" ]]; then
  echo "Set TWINE_PASSWORD or PYPI_API_TOKEN to a PyPI API token, then re-run."
  exit 1
fi

python3 -m pip install -q build twine
rm -rf dist build *.egg-info
python3 -m build
TWINE_USERNAME=__token__ TWINE_PASSWORD="$TOKEN" python3 -m twine upload dist/*
echo "Published aelio-sdk to PyPI."
