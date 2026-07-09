# LL — an AI-native database (working codename)

**One database for relational, vector, full-text, and graph — not four systems stitched
together in application code.** A single record is a row, a vector, a document, and a graph
node at once; one query fuses all four; one transaction makes a write visible across all of
them at the same instant.

Today, an AI application that needs structured filters *and* semantic search *and* full-text
*and* relationship traversal runs Postgres + a vector DB + Elasticsearch + a graph DB, and
joins their results by hand — with no shared transaction, no shared snapshot, and four
systems to operate. LL is the bet that this belongs in **one** engine: one storage substrate,
one write-ahead log, one MVCC model, one cost-based optimizer, one file format. Single-node
first; distribution is an additive later layer.

### Try it in 60 seconds

```sh
cargo run --release -p ll-query --example wedge
```

This runs one query — vector similarity + BM25 text + graph reachability + a scalar filter,
fused — across freshly-inserted (in-memory) and persisted (on-disk) data in a single plan.
No other open-source database answers this in one query. ([what it does, in detail](#the-wedge-one-plan-no-one-else-runs))

To exercise the engine directly:

```sh
cargo test                                                       # the full suite
cargo run --release -p ll-bench -- ./benchdata 50000 128 50 16   # filtered-vector benchmark
```

---

This repository is the **OSS Community Edition** (Apache-2.0): the complete single-node
engine. See [`TechSpec/`](TechSpec/) for the canonical design:

- `LL_Architectural_Specification.md` — full architecture.
- `LL_Decisions_Delta.md` — corrections that supersede the spec (authoritative).
- `LL_FileFormat_ByteLayout.md` — implementation-grade on-disk format.

## Status

Pre-alpha, but the **integration thesis is demonstrated end-to-end**: relational, vector,
full-text, and graph data live in one `.vss` file under one WAL and one MVCC model, and a
single hybrid query is planned and executed across all four modalities and across recent
(memtable) + persisted (segment) data, fused with RRF. The write loop (WAL → memtable →
flush → recovery) and the read loop (catalog → multi-source `Source` fan-out → fusion) are
both closed and tested.

```
write ─► WAL ─► memtable ─► flush(builds indexes) ─► .vss segment
                   │                                      │
query ─► catalog ─►├──────────── Source fan-out ─────────┤─► merge by row_id ─► RRF ─► top-k
                  (memtable)                          (FileSource)
```

- **`crates/ll-format`** — the on-disk file format: **VSS** (Versioned Storage
  Substrate), extension `.vss`.
  - ✅ Step 1: file spine (preamble + TranslationTable section + footer + trailer +
    CRC32C; round-trip & corruption-tested).
  - ✅ Step 2: scalar column chunks (plain encoding, null validity bitmaps, multi-page
    layout with per-page CRC, page index, per-page zone maps in the footer) for Bool /
    I32 / I64 / F32 / F64 / Utf8 / TimestampNanos.
  - ✅ Step 3: f32 vector column chunks (fixed dimension, nulls for pending embeddings,
    per-page centroid + max-radius zone maps for vector pruning).

- **`crates/ll-index`** — index algorithms.
  - ✅ In-memory HNSW (build + search), deterministic (seeded), with recall@k tested
    against brute-force ground truth (recall@10 = 1.0 on 1500×24-d random vectors).
  - ✅ 8-bit scalar quantization (per-dimension) for traversal + full-precision rerank:
    recall@10 = 1.0 (0% loss vs full precision; target was <2%).
  - ✅ On-disk HNSW section (byte-layout §3.1) with BFS reordering: serialize the graph
    to bytes and search over them via `HnswView` — recall@10 = 1.0 preserved across the
    graph → bytes → graph round-trip.
  - ✅ **HNSW embedded in the `.vss` file**: `ll-format` frames opaque index sections, so
    one file holds the vector column, scalar columns, and the HNSW index. An end-to-end
    test writes a `.vss` file, reopens it, and runs filtered vector search entirely from
    the reopened file (recall ≥ 0.95).
  - ✅ **Filtered vector search** — the thesis validation. Three strategies
    (pre/post/integrated-filter) instrumented with a distance-evaluation cost proxy.
    Run `cargo run --release -p ll-index --example filtered_bench`.

- **`crates/ll-text`** — the full-text index (second modality).
  - ✅ Tokenizer + inverted index, **BM25** scoring, boolean AND/OR matching.
  - ✅ On-disk `Text` section (sorted term dictionary + delta/varint postings) with a
    `TextView` that searches over the bytes; embeds in a `.vss` file. An end-to-end test
    writes a `.vss` with a text column + Text section, reopens it, and runs BM25 from the
    file. (FST term dictionary + PFOR-Delta/skip-list postings are later optimizations.)

- **`crates/ll-graph`** — the graph (edge) index (fourth modality).
  - ✅ Forward + reverse CSR adjacency over `(source_local, target_global)` edges, a bloom
    filter over targets (cross-file reverse-traversal pruning), and depth-limited forward
    traversal. On-disk `Edge` section + `EdgeView`; embeds in a `.vss` file. End-to-end
    test runs forward/reverse traversal from a reopened file. (Edge properties and
    per-edge MVCC are later additions.)

- **`crates/ll-wal`** — the write-ahead log (engine spine, in progress).
  - ✅ One LSN-ordered, CRC32C'd log: append logical records
    (`INSERT/UPDATE/DELETE_ROW`, `COMMIT`, `ABORT`, `CHECKPOINT`), fsync at commit, and
    crash-safe replay that stops at the first torn/corrupt record (the crash point).
    Tested against torn-tail and mid-record corruption. (Group commit, file rotation, and
    checkpoint truncation are later additions.)
  - ✅ `open_append`: reopen after recovery, truncating any torn tail.

- **`crates/ll-engine`** — the engine spine (memtable + write loop).
  - ✅ MVCC **memtable** (versioned row store, snapshot-LSN visibility, tombstones).
  - ✅ **Engine** tying WAL ↔ memtable: durable auto-commit writes, crash **recovery**
    (replay committed transactions, reopen the log), and **flush** to a `.vss` file. The
    write loop is closed end-to-end: write → durable in WAL → queryable in memtable →
    survives a crash → persists on flush (all tested).
  - Next: catalog/schema, then the unified query layer across modalities.

- **`crates/ll-catalog`** — the catalog (schema + connective tissue).
  - ✅ Tables with typed columns (scalar / vector / text / edge — telling the planner how
    each is indexed), the `.vss` files per table, column-id assignment, and atomic file
    persistence (in-memory, fully cached). What the planner consults to know what exists
    and which files to fan out to. (redb backing, schema versioning, model registry later.)

- **`crates/ll-query`** — the query layer (in progress).
  - ✅ The `Source` abstraction: per-modality search (vector / text-BM25 / graph) returning
    `(global_row_id, score)` candidates at a snapshot LSN. The global row id is the join
    key that unifies recent and persisted data.
  - ✅ Implemented for **both** the memtable *and* flushed `.vss` files (`FileSource`,
    querying the embedded HNSW/text/edge sections with `xmin`-column MVCC). Recent and
    persisted data are queryable through one interface, across all four modalities —
    **read-your-writes fully closed**. Engine flush now builds the index sections.
  - ✅ **Unified hybrid executor** — first-class operators (vector / text / graph /
    scalar filter), fanned out to every `Source`, merged by global `row_id`, fused with
    **Reciprocal Rank Fusion**. The `Database` facade ties catalog + engine + segments into
    one `query()` over named tables. **"One query plan" — caveat 2 closed.**
  - ✅ **Cost model wired into execution**: a vector query with a scalar filter now has
    its strategy (pre-filter vs post-filter) chosen by `ll-cost` from the estimated
    selectivity — so selective filtered queries get **exact recall** instead of the
    post-filter collapse. `explain()` reports the chosen strategy. **"One cost model" —
    the last conceptual piece of the thesis.**
  - ✅ **Mutations + compaction**: `Database::delete` / `update` write through the engine at
    a fresh LSN, so the global newest-version-wins resolver suppresses the old copy even when
    it lives in a flushed segment (`update` reconstructs the full row from whichever source
    holds it). `Database::compact` merges every segment + the memtable into one fresh,
    fully-reindexed segment, physically dropping tombstones — crash-safe (fsync segment →
    atomic manifest swap → WAL truncate → unlink old files).
  - Next (refinements): histogram-based selectivity (avoid the scan), explicit
    transactions, background compaction scheduling, and a DataFusion swap for SQL + the
    relational tail.

- **`crates/ll-cost`** — the cost model.
  - ✅ Equi-height `Histogram` for scalar selectivity estimation (±0.02 on uniform data).
  - ✅ Filtered-vector strategy **selector**: estimate selectivity → predict each
    strategy's cost + recall → choose the cheapest meeting the recall bar. An
    integration test (`tests/optimizer.rs`) confirms the model's choice **matches the
    empirically optimal strategy** across the selectivity sweep on the real index, with
    `hnsw_base_evals` calibrated from actual searches.
  - ✅ **Vector cardinality** via per-file centroid distance distributions (`VectorStats`)
    — the spec's novel estimator (§Part X): estimate the fraction of vectors within
    distance T of a query. Sampled data-point centroids (query-like; cluster *means*
    regress to the center in high-d and over-estimate), each with a distance histogram;
    nearest centroid answers the query. Accurate at high selectivity (MAE ≈ 0.02) and
    discriminative across the range. Run
    `cargo run --release -p ll-cost --example vector_cardinality`.
  - Next: the full microsecond-calibrated operator cost model; wiring stats into planning.

### Filtered-vector benchmark (the core result)

`examples/filtered_bench.rs` sweeps predicate selectivity over 5000×32-d vectors. The
finding that justifies a cost-based optimizer — **no single strategy wins everywhere**:

| selectivity (≈ rows matching) | cost-optimal strategy | note |
|---|---|---|
| ≤ 0.25 (≲1,400 rows) | **pre-filter** | exact (recall 1.0), fewest distance evals |
| ≥ 0.5 | **post-filter** | HNSW candidate cost (~1,400) beats scanning all matches |

Two observations:
- **Post-filter recall collapses at low selectivity** (recall ≈ 0.02 at selectivity 0.002):
  this is the failure mode of naive `ORDER BY dist LIMIT k ... WHERE` vector search.
  **Integrated-filter stays at recall 1.0** but pays full-graph traversal there.
- The pre-filter ⇄ HNSW crossover lands at **~1,300–1,400 matching rows**, matching the
  architecture spec's predicted ~1,500. Selectivity estimation is therefore the whole
  game — exactly what the per-file centroid-distance cost model is built to provide.

## Correctness

Beyond per-feature tests, the suite includes **deterministic model-based simulation**
(`ll-engine/tests/sim.rs`): random sequences of insert/update/delete/crash-recover/flush
are checked against a reference oracle across many seeds — the TigerBeetle/FoundationDB
discipline. And **parser robustness fuzzing**: the *entire read path* is panic-proof on malformed
input — `read_file` (`ll-format`) and every index view (`HnswView`/`TextView`/`EdgeView`)
survive thousands of random, mutated, and truncated byte inputs without panicking
(checked reads, validated regions, bounded loops).

## Benchmarks

`cargo run --release -p ll-query --example query_bench` — filtered vector search at 20k×32-d
through the full query path, vs exact filtered ground truth. **OURS** = the unified executor
(cost model auto-picks pre/post-filter); **NAIVE** = HNSW top-k then filter (what a vector
DB + `WHERE` does):

| selectivity | matched | strategy | OURS recall | NAIVE recall |
|---|---|---|---|---|
| 0.005 | 120 | PreFilter | **1.000** | 0.047 |
| 0.02 | 435 | PreFilter | **1.000** | 0.237 |
| 0.05 | 1061 | PreFilter | **1.000** | 0.513 |
| 0.10 | 2139 | PreFilter | **1.000** | 0.870 |
| 0.25 | 5051 | PostFilter | **1.000** | 1.000 |
| 1.00 | 20000 | PostFilter | **1.000** | 1.000 |

The differentiator, quantified: **filtered vector search stays correct at every selectivity**
because the cost model picks the right strategy, while the naive approach collapses to ~5%
recall when the filter is selective.

**Recall-aware, adaptive planning.** Post-filter (ANN) recall is bounded by the index's
*unfiltered* recall, which falls with dimensionality — at high dim ANN simply can't return the
true top-k. So at flush LL **measures a recall@k-vs-`ef` curve** for each vector index (probed
across `ef ∈ {128…2560}`, self-excluded, then discounted by a safety margin for out-of-sample
queries) and stores it in the OptimizerStats section. The planner then, per query:

1. picks the **smallest `ef`** whose measured recall meets the target — *search harder* before
   giving up;
2. weighs that post-filter cost against an **exact pre-filter** (a true-distance scan of the
   matching rows, always recall 1.0) and takes the cheaper;
3. falls back to exact pre-filter when **no `ef`** can meet the target.

So recall never silently drops below target — it's traded for latency instead — and EXPLAIN
shows the chosen strategy, the `ef`, and why. On `cargo run --release -p ll-bench` (50k, k=10,
target 0.9):

| dataset | strategy | query recall |
|---|---|---|
| 50k×32  (m=16) | post-filter at high sel, pre-filter at low | **1.000** all selectivities |
| 50k×128 (m=16) | post-filter @ef≈640 at high sel (~3–5ms), pre-filter at low | **0.94–1.00** |
| 50k×384 (m=16) | exact pre-filter; post-filter only where a wide `ef` still wins | **0.97–1.00** |
| 50k×384 (m=32) | post-filter @ef≈640 viable at high sel | **0.94–1.00** |
| 50k×768 (m=16) | exact pre-filter everywhere (ANN can't meet target) | **1.000** all selectivities |

Raising the index's `m` lifts the whole recall curve (e.g. 384-d crosses the target at ef≈640
instead of needing near-exact), widening the fast post-filter envelope at the cost of index
size. Other perf: distance kernels (f32 + 8-bit) are **SIMD** (AVX2+FMA, scalar fallback);
selectivity is estimated by **sampling**; a **sorted scalar index** makes pre-filter
O(log N + matches) on segments; sources are `Sync` so concurrent queries scale across cores
(~4–6× on 12 threads); the `Database` keeps flushed segments resident as `FileSource`s.
Remaining: per-query sampling still allocates, and HNSW build is single-threaded — neither
affects recall.

### Realism sweep

`ll-bench <dir> <n> <dim> <queries> <m> [corr] [segments]` exercises the planner across the
axes that matter. Across all of them, **query recall never drops below the 0.9 target** — the
planner uses fast post-filter where the ANN can meet it and exact pre-filter otherwise:

- **Scale** 100k×128 — post-filter @ef1280, recall 0.93–1.0.
- **Dimension** 50k×1536 — ANN can't meet target at any `ef`, so exact pre-filter everywhere, recall 1.0.
- **Correlated filter** (`corr`: the predicate selects a spatial slab, not a random subset) —
  recall 0.94–1.0; the adaptive `ef` + over-fetch absorb the correlation.
- **Multiple segments** (`segments N`: data split across N `.vss` files) — the executor merges
  per-segment ANN results by global row id; recall 0.99–1.0 (smaller per-segment HNSWs recall
  better), validating cross-segment search under load.

### Head-to-head vs pgvector and Qdrant

A shared-dataset, common-schema harness compares LL against real vector DBs on filtered
search. LL's runner (`cargo run --release -p ll-bench`) generates the dataset and writes
`results/ll.json`; `bench/pgvector_bench.py` and `bench/qdrant_bench.py` load the *same*
dataset into containers and emit the same schema; `bench/compare.py` prints them side by side.
See [bench/BENCHMARKS.md](bench/BENCHMARKS.md) for methodology.

Result (50k×128, k=10, each system out-of-the-box; pgvector 0.8.2/pg16, Qdrant latest):

```
RECALL@k          ll    pgvector      qdrant
  sel 0.005    1.000       1.000       1.000
  sel 0.02     1.000       0.077       1.000
  sel 0.05     1.000       0.201       1.000
  sel 0.10     1.000       0.285       1.000
  sel 0.25     0.977       0.323       1.000
  sel 0.50     0.939       0.352       1.000
  sel 1.00     0.964       0.403       1.000
```

- **vs pgvector** — the thesis, confirmed: its default filtered query (`WHERE cat<t ORDER BY
  emb <-> q`) post-filters and **collapses** (recall 0.08–0.40 across the moderate range, the
  classic "ANN-then-filter" failure). LL stays at 0.94–1.0 because the cost model pre-filters
  when the predicate is selective. (pgvector 0.8's `hnsw.iterative_scan` mitigates this but is
  off by default — these are out-of-the-box numbers.)
- **vs Qdrant** — Qdrant's filterable-HNSW holds recall 1.0; LL matches it at low/mid
  selectivity and is within ~0.05 at high selectivity (where LL's post-filter is approximate —
  a higher index `m` closes the gap). So LL is **competitive with a best-in-class purpose-built
  vector DB on filtered recall**, at v0, single-node, zero dependencies.

**At scale (50k → 100k → 250k×128)** the picture sharpens. pgvector's filtered recall stays
collapsed (and worsens — ~0.1–0.18 at 250k). Qdrant holds 1.0 *while the filter is selective*,
but at 250k its default-`ef` HNSW can no longer keep recall@10 over the near-unfiltered set, so
it **drops to ~0.70–0.79 at high selectivity** — the same scale/dimensionality ceiling LL's
post-filter would hit, showing up at the *other* end of the selectivity axis. LL holds **1.0 at
every selectivity and every scale** here, because when its measured curve says ANN can't meet
the target it falls back to the **exact** pre-filter:

```
recall @ sel=1.0 (unfiltered)   50k     100k    250k
  LL                            0.964   0.953   1.000   (post-filter / exact fallback)
  pgvector                      0.403   0.282   0.172   (post-filter collapse)
  Qdrant                        1.000   1.000   0.698   (default-ef HNSW ceiling at scale)
```

That perfect recall isn't free: LL's exact fallback is a scan, ~50ms at 250k/sel=1.0 — the
honest cost, and the signal to raise the index `m` (which lifts the curve so the fast
post-filter stays viable). The takeaway is the *shape*: LL is the only one of the three whose
recall never silently drops — it trades latency, not correctness.

**Latency is not directly comparable** and is omitted from the headline: LL is measured
in-process (no wire protocol yet), while pgvector and Qdrant answer over their client
protocols (SQL / HTTP + serialization). Recall is system-intrinsic and *is* comparable — it's
the point. All three use default search parameters; each can be tuned further (a higher Qdrant
`ef` recovers its high-selectivity recall; a higher LL `m` removes its exact-scan fallback).

### The wedge: one plan no one else runs

Vector-filter recall is where LL *ties* the best (Qdrant). The reason LL exists is the query
those systems **can't** answer in one plan. `cargo run --release -p ll-query --example wedge`
runs a single [`HybridQuery`] that fuses **all four modalities at once** — vector + BM25 text +
graph reachability + a scalar filter — and spans the persisted segment *and* the in-memory
memtable, so brand-new un-flushed rows participate with no reindex:

```
EXPLAIN: HybridRank[RRF k=60] over [VectorSearch, TextMatch] filter[year] PathReachable(cites, depth 2) across 2 sources
"of the papers my just-submitted preprint cites (transitively), which are about 'diffusion',
 near my topic vector, and published since 2020?"  → fused top-k across memtable + segment
```

The graph traversal crosses the memtable→segment boundary (the new preprint cites both a
persisted paper and another just-inserted one); the result mixes persisted and
read-your-writes rows in one fused ranking. No mainstream system does this in a single plan:
pgvector has vector+SQL but no graph hops or fused BM25; Qdrant has vector+payload filters but
no joins, BM25 fusion, or traversal. The real-world alternative is Postgres + Qdrant + a text
engine + a graph store joined in application code — with no cross-store consistency and no
read-your-writes. LL does it over one substrate, one WAL, one MVCC model.

## Run it as a server

LL embeds as a Rust library, but `ll-server` exposes a single-node database over HTTP/JSON —
the same engine, reachable over the wire (the managed-cloud entry point):

```sh
LL_API_KEYS=secret LL_DATA_DIR=./lldata cargo run --release -p ll-server
# listens on 127.0.0.1:8080 (override with LL_BIND); open mode if LL_API_KEYS is unset
```

```sh
# create a table, insert a row, run a hybrid query
curl -XPOST localhost:8080/v1/tables -H 'authorization: Bearer secret' -H 'content-type: application/json' \
  -d '{"name":"docs","columns":[{"name":"embedding","kind":"vector","dim":2},{"name":"body","kind":"text"},{"name":"year","kind":"i64"}]}'

curl -XPOST localhost:8080/v1/tables/docs/rows -H 'authorization: Bearer secret' -H 'content-type: application/json' \
  -d '{"values":{"embedding":{"type":"vector","value":[0,0]},"body":{"type":"utf8","value":"alpha diffusion"},"year":{"type":"i64","value":2021}}}'

curl -XPOST localhost:8080/v1/tables/docs/query -H 'authorization: Bearer secret' -H 'content-type: application/json' \
  -d '{"k":5,"vector":{"col":"embedding","query":[0,0]},"text":{"col":"body","query":"diffusion"},"filters":[{"col":"year","op":"ge","value":{"type":"i64","value":2019}}]}'
```

Endpoints (all under `/v1`, bearer-auth except `/health`): `POST /tables`, `GET
/tables/{t}/schema`, `POST|PATCH|DELETE /tables/{t}/rows[/{id}]`, `POST /tables/{t}/query`,
`POST /tables/{t}/explain`, `POST /tables/{t}/nl`, `POST /admin/flush`, `POST /admin/compact`.
TLS and per-tenant isolation are reverse-proxy / later concerns; gRPC and a SQL frontend are
planned on top of the same `Database`.

### Ask in English

Set `ANTHROPIC_API_KEY` and the `/nl` endpoint compiles natural language into a structured
hybrid query, grounded in the table's real schema (it can only use columns that exist), runs
it, and echoes the compiled query back for transparency:

```sh
curl -XPOST localhost:8080/v1/tables/docs/nl -H 'authorization: Bearer secret' -H 'content-type: application/json' \
  -d '{"query": "diffusion papers since 2019"}'
# → {"compiled": {"k":10,"text":{"col":"body","query":"diffusion"},
#                 "filters":[{"col":"year","op":"ge","value":{"type":"i64","value":2019}}]},
#    "results": [ ... ]}
```

NL is a thin, schema-grounded translation step — not a storage mode — and lowers onto the same
`HybridQuery` IR as the SDK and (later) SQL.

### Semantic search without computing embeddings

Set `LL_EMBED_KEY` (and optionally `LL_EMBED_URL` / `LL_EMBED_MODEL` — any OpenAI/Voyage-style
`/embeddings` endpoint) and the server embeds text for you, on both sides:

```sh
# insert: send text to embed instead of a raw vector
curl -XPOST localhost:8080/v1/tables/docs/rows -H 'authorization: Bearer secret' -H 'content-type: application/json' \
  -d '{"values":{"embedding":{"type":"embed","value":"a paper on latent diffusion"},"year":{"type":"i64","value":2023}}}'

# query: semantic similarity by meaning — the server embeds the text into a query vector
curl -XPOST localhost:8080/v1/tables/docs/query -H 'authorization: Bearer secret' -H 'content-type: application/json' \
  -d '{"k":5,"semantic":{"col":"embedding","text":"image generation models"}}'
```

With an embedder configured, `/nl` also gains a `semantic` clause, so "papers *about* X" maps
to vector similarity, not just keyword match. The embedding model must produce vectors matching
the column's dimension — the server **enforces** this: a mismatch (or `embed`/`semantic` aimed
at a non-vector column) returns a clear `400`, rather than silently dropping the vector and
returning nothing. Set `LL_EMBED_MODEL` so its output dimension equals the column's `dim`
(`voyage-3-lite` → 512, `voyage-3` → 1024, OpenAI `text-embedding-3-small` → 1536).

### Docker

```sh
docker build -t ll-server .
docker run -p 8080:8080 -v ll-data:/data \
  -e LL_API_KEYS=secret \
  -e ANTHROPIC_API_KEY=sk-...   `# enables /nl` \
  -e LL_EMBED_KEY=...           `# enables semantic search + embed-on-insert` \
  ll-server
```

## Build

```sh
cargo test
```
