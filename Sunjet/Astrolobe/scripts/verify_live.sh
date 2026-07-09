#!/usr/bin/env bash
# Live verification of the embedding + natural-language paths against the real provider APIs.
#
# Usage:
#   ANTHROPIC_API_KEY=sk-ant-... LL_EMBED_KEY=pa-... ./scripts/verify_live.sh
#
# Optional:
#   LL_EMBED_MODEL  embedding model (default voyage-3-lite)
#   DIM             vector column dimension — MUST equal the model's output dim
#                   (voyage-3-lite=512, voyage-3=1024, OpenAI text-embedding-3-small=1536)
#   LL_EMBED_URL    embeddings endpoint (default Voyage; any OpenAI-style works)
#   PORT            listen port (default 8088)
#
# Exits non-zero on the first failed check.
set -euo pipefail

PORT="${PORT:-8088}"
DIM="${DIM:-512}"
export LL_EMBED_MODEL="${LL_EMBED_MODEL:-voyage-3-lite}"
export LL_BIND="127.0.0.1:${PORT}"
export LL_DATA_DIR="$(mktemp -d)"

: "${ANTHROPIC_API_KEY:?set ANTHROPIC_API_KEY to verify /nl}"
: "${LL_EMBED_KEY:?set LL_EMBED_KEY to verify semantic search}"

B="http://127.0.0.1:${PORT}/v1"
H=(-H 'content-type: application/json')

echo "building ll-server..."
cargo build --release -p ll-server >/dev/null 2>&1

./target/release/ll-server &
SRV=$!
trap 'kill "$SRV" 2>/dev/null || true; rm -rf "$LL_DATA_DIR"' EXIT

# Wait for health.
for _ in $(seq 1 40); do
  curl -sf "${B}/health" >/dev/null 2>&1 && break
  sleep 0.5
done
curl -sf "${B}/health" >/dev/null || { echo "FAIL: server did not become healthy"; exit 1; }
echo "PASS: server healthy"

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; echo "  response: $2"; exit 1; }

# 1. Create a table whose vector dim matches the embedding model.
curl -sf "${H[@]}" -XPOST "${B}/tables" -d "{
  \"name\":\"docs\",
  \"columns\":[
    {\"name\":\"emb\",\"kind\":\"vector\",\"dim\":${DIM}},
    {\"name\":\"body\",\"kind\":\"text\"},
    {\"name\":\"year\",\"kind\":\"i64\"}
  ]}" >/dev/null || { echo "FAIL: create table"; exit 1; }
pass "create table (vector dim ${DIM})"

# 2. embed-on-insert: send text, the server embeds it. Row 1 = diffusion, row 2 = unrelated.
r=$(curl -s "${H[@]}" -XPOST "${B}/tables/docs/rows" -d '{"values":{
  "emb":{"type":"embed","value":"latent diffusion models for high-resolution image synthesis"},
  "body":{"type":"utf8","value":"diffusion image generation"},
  "year":{"type":"i64","value":2022}}}')
echo "$r" | grep -q '"row_id"' || fail "embed-on-insert (diffusion)" "$r"
pass "embed-on-insert -> $r"

curl -s "${H[@]}" -XPOST "${B}/tables/docs/rows" -d '{"values":{
  "emb":{"type":"embed","value":"a traditional recipe for sourdough bread"},
  "body":{"type":"utf8","value":"baking bread"},
  "year":{"type":"i64","value":2019}}}' >/dev/null

# 3. Semantic search: a meaning-similar query should rank the diffusion row (id 1) first.
r=$(curl -s "${H[@]}" -XPOST "${B}/tables/docs/query" \
  -d '{"k":2,"semantic":{"col":"emb","text":"generating images with neural networks"}}')
top=$(printf '%s' "$r" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("results",[{}])[0].get("row_id","none"))' 2>/dev/null || echo parse_error)
[ "$top" = "1" ] || fail "semantic search did not rank the diffusion row first (got top=$top)" "$r"
pass "semantic search ranked the diffusion row first -> $r"

# 4. Natural language: compiles to a structured query and runs it.
r=$(curl -s "${H[@]}" -XPOST "${B}/tables/docs/nl" \
  -d '{"query":"papers about diffusion published since 2020"}')
printf '%s' "$r" | python3 -c 'import sys,json; d=json.load(sys.stdin); assert "compiled" in d and "results" in d, d' 2>/dev/null \
  || fail "NL query did not return a compiled query + results" "$r"
pass "natural-language query compiled and ran -> $r"

echo
echo "ALL LIVE CHECKS PASSED"
