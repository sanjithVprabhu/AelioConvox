# Prism Query Language v0

Prism is Aelio DB's closed, multimodal read language. A Prism request is data, not executable
query text: there are no joins, subqueries, computed expressions, user-defined functions, or raw
SQL fragments.

The authenticated HTTP entry point is `POST /v1/prism` on `aelio-server`. The same implementation
is available in Rust through `aelio_query::parse_prism` and
`aelio_db_query::execute_prism`.

## Envelope

```json
{
  "from": "prompts",
  "match": [
    {"kind": "text", "on": "body", "query": "customer reply"},
    {"kind": "vector", "on": "embedding", "embed": "customer reply"},
    {"kind": "fusion", "method": "rrf", "rrf_k": 60}
  ],
  "where": [
    {"col": "active", "op": "eq", "value": true}
  ],
  "select": ["id", "body", "version"],
  "limit": 10,
  "into": "candidates"
}
```

`select` and `limit` are mandatory. A query must contain at least one `match` or `where`
predicate, so a bare table dump is not a valid Prism program. `into` is optional metadata for a
Mother DSL/kernel caller; the database HTTP response is returned directly.

## Closed match clauses

- `key`: `{"kind":"key","on":"id","value":"p1"}`
- `text`: `{"kind":"text","on":"body","query":"terms"}`
- `vector`: exactly one of `vector` or `embed`
- `graph`: requires `seeds`, `max_depth`, and `max_nodes`
- `fusion`: RRF only; multimodal ranked queries require one explicit fusion clause

Every clause rejects unknown fields. Prism v0 allows at most one text, one vector, one graph, and
one fusion clause. Scalar `where` operators are `eq`, `ne`, `gt`, `ge`, `lt`, and `le`.

## Hard limits and failure behavior

- Output limit: `1..=10,000`
- Match clauses: at most 8
- Where predicates: at most 64
- Selected columns: at most 256
- Graph seeds: at most 64
- Graph depth: `1..=32`
- Graph visited nodes: `1..=10,000`
- Names: at most 255 UTF-8 bytes, non-empty, no control characters
- Text/embed input: at most 65,536 UTF-8 bytes
- Whole envelope: Sol structural limits and a 1 MiB canonical representation

Limits are checked again during lowering, so constructing `PrismQuery` directly cannot bypass
the parser. Graph-budget exhaustion is an explicit error, never an empty successful result.

## Schema and data safety

Before execution, Prism verifies all projection, predicate, and match columns against the physical
Aelio DB catalog. Match modalities must agree with column kinds, predicate values must have the
declared scalar type, query vectors must contain finite values of the exact declared dimension,
and embedding results are dimension-checked before search.

Only selected fields are returned. A misspelled projection fails closed instead of silently
returning a partial row. Aelio DB also rejects wrong-typed cells, wrong-dimensional or non-finite
vectors, duplicate input columns, oversized text cells, and oversized edge lists at write time so
bad data cannot poison Prism indexes.

## Ranking

Text and vector rankings are fused with Reciprocal Rank Fusion. Prism fuses ranks rather than raw
scores because BM25 and vector distances are not directly comparable. Graph is a reachability
constraint and scalar predicates filter the candidate set.

## Verification

The core suites are:

```bash
cargo test --manifest-path aelio-os/Cargo.toml -p aelio-query -p aelio-db-query
cargo test --manifest-path aelio-os/Cargo.toml -p aelio-db-api
```

`prism_adversarial.rs` covers parser closure, bounds, tenant-scoped schema checks, modality/type
mismatches, projection non-leakage, graph budgets, embedding dimensions, updates, deletes,
flush/compaction, fusion, physical schema validation, and poisoned-row rejection.
