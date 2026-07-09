# Benchmarks — filtered vector search, head to head

This harness measures **filtered vector search** — `ORDER BY embedding <-> q WHERE attr < t
LIMIT k` — across LL and other vector databases, on **one shared dataset** with **one
results schema**, so the comparison is apples to apples.

We benchmark filtered search specifically because it is (a) extremely common in real apps
(RAG with metadata filters, multi-tenant, e-commerce) and (b) where the integrated design
differentiates: a system that picks the retrieval strategy by selectivity stays accurate,
while the naive "ANN then filter" approach collapses when the filter is selective.

## Metric

For each selectivity, each system runs the same queries and we report, vs **exact filtered
ground truth** (brute force over the matching rows):

- **recall@10** — the headline. Did you return the true nearest matching rows?
- **latency** — ms/query (informative; LL v0 is single-threaded / unoptimized — see caveats).

## Shared dataset format (little-endian)

A dataset directory contains:

- `base.fvecs` — N base vectors. fvecs = repeated `[dim: int32][dim × float32]`.
- `query.fvecs` — the query vectors, same format.
- `attr.u32` — N × `uint32`, one filter attribute per base row (values in `0..1000`).

Row id = position in `base.fvecs` (0-based). The predicate at selectivity `s` is
`attr < round(s * 1000)`, so ~`s·N` rows match. Every runner computes its own exact ground
truth from these files (deterministic — identical across systems).

Generate a synthetic dataset (and run LL) with:

```sh
cargo run --release -p ll-bench -- ./benchdata 100000 32 100
#                                    dir         N      dim Q
```

Real ANN datasets (SIFT1M, GloVe) are already in fvecs and drop in directly — supply your
own `attr.u32` (e.g. random categories) to define the filter.

## Results schema (`<dir>/results/<system>.json`)

```json
{ "system": "ll", "dataset": "100000x32", "k": 10,
  "results": [ { "selectivity": 0.01, "matched": 1000,
                 "strategy": "PreFilter", "recall": 1.0, "latency_ms": 0.4 }, ... ] }
```

## Running each system

All read the same `<dir>` and write `<dir>/results/<system>.json`.

```sh
# LL (this repo)
cargo run --release -p ll-bench -- ./benchdata

# pgvector  (needs Docker + Postgres with pgvector)
docker run -d --name pgv -e POSTGRES_PASSWORD=pw -p 5432:5432 pgvector/pgvector:pg16
pip install psycopg2-binary numpy
python bench/pgvector_bench.py ./benchdata "host=localhost user=postgres password=pw"

# Qdrant  (needs Docker)
docker run -d --name qdrant -p 6333:6333 qdrant/qdrant
pip install qdrant-client numpy
python bench/qdrant_bench.py ./benchdata http://localhost:6333
```

Then compare:

```sh
python bench/compare.py ./benchdata
```

## Fairness notes

- Same dataset, same queries, same predicate, same k, same exact ground truth for everyone.
- Each system uses its **native** filtered-search path (pgvector: `WHERE … ORDER BY <->`;
  Qdrant: filtered search; LL: the cost-model executor). We are comparing *what each system
  actually does* for a filtered query, not a hand-tuned workaround.
- Build each index with its recommended defaults; note any index params in the results.

## Honest caveats for LL (v0)

- Distance kernels are SIMD-accelerated (AVX2+FMA, with a scalar fallback) for both the f32
  rerank and the 8-bit-quantized HNSW traversal; CRC is still software. Within-query work is
  single-threaded — concurrent *throughput* scales across cores (the `Source: Sync` design),
  but a single query's latency is not yet as tuned as mature systems.
- LL estimates selectivity by **sampling** up to 2048 rows per source (not a full scan); a
  stored `ll-cost` histogram (built & tested) would remove even the sample in production.
- The point of this harness is **recall under filtering** (correctness), where LL's design
  wins; treat single-query latency as directional until further optimization lands.

## Recorded result (50k×128, k=10, defaults)

Run on this repo's harness — pgvector 0.8.2/pg16 and Qdrant (latest) in Docker, all
out-of-the-box search params:

| selectivity | LL recall | pgvector recall | Qdrant recall |
|---|---|---|---|
| 0.005 | 1.000 | 1.000 | 1.000 |
| 0.02  | 1.000 | 0.077 | 1.000 |
| 0.05  | 1.000 | 0.201 | 1.000 |
| 0.10  | 1.000 | 0.285 | 1.000 |
| 0.25  | 0.977 | 0.323 | 1.000 |
| 0.50  | 0.939 | 0.352 | 1.000 |
| 1.00  | 0.964 | 0.403 | 1.000 |

- **pgvector** collapses under filtering (post-filter ANN); its 0.8 `hnsw.iterative_scan`
  (off by default) would help. **Qdrant** (filterable-HNSW) holds 1.0; **LL** matches it except
  at high selectivity where its post-filter is approximate (a higher index `m` closes that).
- **Latency** is intentionally not tabulated as a headline: LL runs in-process while pgvector
  and Qdrant answer over their client protocols (SQL / HTTP + serialization), so the numbers
  aren't comparable. Recall is system-intrinsic and is the comparison that matters.

### Scaling (50k → 100k → 250k×128, defaults)

Recall@10 at three selectivities as N grows:

| N | sel | LL | pgvector | Qdrant |
|---|---|---|---|---|
| 50k  | 0.05 | 1.000 | 0.201 | 1.000 |
| 50k  | 0.50 | 0.939 | 0.352 | 1.000 |
| 50k  | 1.00 | 0.964 | 0.403 | 1.000 |
| 100k | 0.05 | 1.000 | 0.193 | 1.000 |
| 100k | 0.50 | 0.955 | 0.262 | 1.000 |
| 100k | 1.00 | 0.953 | 0.282 | 1.000 |
| 250k | 0.05 | 1.000 | 0.108 | 1.000 |
| 250k | 0.50 | 1.000 | 0.182 | 0.794 |
| 250k | 1.00 | 1.000 | 0.172 | 0.698 |

- **pgvector** stays collapsed and worsens with scale.
- **Qdrant** holds 1.0 while the filter is selective, but at 250k its default-`ef` HNSW can't
  keep recall@10 over the near-unfiltered set (drops to ~0.70 at sel=1.0). A higher `ef`
  recovers it.
- **LL** holds 1.0 at every selectivity/scale here by falling back to the exact pre-filter when
  the measured ANN curve can't meet the target — at the cost of an exact-scan latency
  (~50ms at 250k/sel=1.0). A higher index `m` lifts the curve so the fast post-filter stays
  viable instead.

All three at default search params; latency not compared (LL in-process vs client protocols).

Reproduce: `cargo run --release -p ll-bench -- ./cmpdata <N> 128 50 16`, start the two
containers (commands at the top of `pgvector_bench.py` / `qdrant_bench.py`), run both Python
runners against `./cmpdata`, then `python3 compare.py ./cmpdata`.
