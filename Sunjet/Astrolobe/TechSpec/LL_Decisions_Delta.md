# LL — Decisions Delta (Amendments to the Architectural Specification)

This document records corrections and commitments made **after** the canonical
`LL_Architectural_Specification.md` was written. Where this document and the spec
disagree, **this document wins**. Each entry notes the date and reason.

---

## D-001 — Row identity model (corrects Part III)
**Date:** 2026-06-07 · **Reason:** `(file_id, local_offset)` cannot be stable across
compaction (compaction assigns a new `file_id`), and a file-coupled ID also breaks
under sharding.

**Decision — three-layer identity:**
- **Global row ID:** `row_id: u64`, assigned at insert / WAL-append time. Stable for
  the life of the row, across updates, compaction, and (future) sharding. This is the
  *only* identity that is "stable forever."
- **File-local offset:** `u32`, dense within a file (0..N-1). Used internally by the
  file's indexes (HNSW node IDs, CSR offsets, posting list doc IDs) for compactness.
  **Not stable** — reassigned on every flush/compaction.
- **Footer translation table:** every file maps `local_offset <-> global row_id`
  (already specified in the Part V footer).

**Storage rules:**
- Edge columns and any cross-file / external reference store the **global `u64`**.
- HNSW / CSR / posting lists store **local `u32`** offsets; translate via the footer.
- On compaction: local offsets are reassigned, global row IDs preserved, translation
  tables rebuilt, indexes rebuilt against new local offsets.

**Consequence:** the Part III prose calling `(file_id, local_offset)` "the stable
internal row ID" is **deleted**. Part XIX's "stable row IDs" primitive refers to the
global `u64`.

---

## D-002 — Embedding write contract (corrects Part III line ~121)
**Date:** 2026-06-07 · **Reason:** "A failed embed rolls back the row" contradicts the
default deferred mode, which commits the row with `embedding = NULL`.

**Decision — two explicit modes; embedding atomicity stated precisely:**
- **Deferred (default):** INSERT/UPDATE commits immediately with `embedding = NULL`,
  `embed_status = pending`. Embedding generation is **outside** the write
  transaction — the WAL does not gate commit on the embed API. Vector visibility for
  the row arrives later, when the background worker populates it (a new MVCC version).
- **Inline (opt-in per table):** embedding is generated **synchronously before
  commit**. An embedding failure **fails the write**. The committed row always has a
  populated embedding; there is no `pending` state.

**Consequence:** the sentence "A failed embed rolls back the row" in Part III is
**deleted**. Rollback semantics apply only to inline mode, and only because the embed
runs *before* the commit point — not because embedding participates in WAL atomicity.

---

## D-003 — License and edition split (fills a Part XXI "decision outside this document")
**Date:** 2026-06-07

- **OSS Community Edition — Apache 2.0.** The *complete* single-node engine: file
  format, WAL, memtable, MVCC, compaction, buffer pool, catalog, vector/text/edge
  query, cost-based optimizer, DataFusion integration, embedding lifecycle, SDKs,
  EXPLAIN/observability. Not a crippled demo.
- **Commercial Edition / Cloud.** Managed cloud service (primary commercial vehicle,
  available against single-node from day one), plus: read replicas & HA, sharding,
  multi-region, enterprise auth (OAuth/OIDC, mTLS, SSO), admin UI, hash-chained audit,
  application-level encryption at rest, operational automation, support.
- **Naming honesty:** if a source-available license (e.g. BSL) is ever adopted for any
  component, it will **not** be described as "open source."

---

## D-004 — v0 scope (ruthless narrowing of Part XXI "v1")
**Date:** 2026-06-07 · **Reason:** Spec's "v1" is really v1 + v1.5 + production platform.
The first artifact must *prove the thesis*, not be feature-complete.

**v0 goal:** demonstrate that a single file / WAL / MVCC / query engine executes a
hybrid **vector + text + scalar** query better than glued-together systems — with the
filtered-vector cost model as the centerpiece. **This proof is the company.**

**v0 build path (each step gated by a benchmark/test before the next):**
1. Immutable file format: scalar + vector columns, footer, three-layer checksums.
2. WAL + memtable + flush + crash recovery.
3. Per-file HNSW + scalar predicate filtering — **GO/NO-GO GATE: benchmark filtered
   vector search (pre/post/integrated-filter) hard; this is the differentiator.**
4. Text: FST + PFOR-Delta posting lists.
5. Edge CSR with depth-limited traversal (graph — see open question below).
6. DataFusion integration: first unified logical/physical operators.
7. Embedding lifecycle (EMBED FROM, durable job queue, cache).
8. Only then: Postgres wire, polished SDKs, Helm, Terraform, managed-cloud plumbing.

**Cross-cutting disciplines (start at step 1, not later):**
- Deterministic simulation testing (TigerBeetle/FoundationDB style) for storage + WAL.
  Trust is the actual product; it is earned only this way.
- At least one design partner with a real workload before step 5.

**v0 modality scope (resolved 2026-06-07):** all four modalities, **including graph**,
are in v0. Rationale: stay true to the integration thesis and front-load the hardest
storage problem (edge/CSR-in-LSM) rather than discover it late. The open market
question below still stands and should be answered with a design partner.

**Open question (still unresolved):** is the **graph leg** load-bearing for the first
customers, or thesis-driven? Validate against a real workload before over-investing.

---

## D-005 — Translation table is a section, not inlined in the footer (corrects §5 of byte-layout)
**Date:** 2026-06-07 · **Reason:** at large tier files `row_count × 8` is tens-to-
hundreds of MB; inlining it in the FlatBuffers footer makes the footer un-hot (the
whole buffer must materialize to read any field).

**Decision:** the `local_offset -> global_row_id` map is a dedicated
`SectionType.TranslationTable` section, referenced by a footer `SectionEntry` carrying
`{offset, length, encoding, crc32}`. Body = `u64[row_count]` indexed by `local_offset`.
v0 encoding = `Plain`; later `Delta`/`Pfor` (within-file global IDs are near-monotonic,
so delta compresses to ~1 B/row). Buffer pool pins it for small files, lazily loads for
large ones.

---

## D-006 — Split `hnsw_node_id` from `local_offset` (corrects byte-layout §3.1)
**Date:** 2026-06-07 · **Reason:** HNSW BFS reordering reorders *node IDs* for graph
locality; if node ID ≡ `local_offset`, achieving that locality would require reordering
the whole file's row order to one vector index's BFS order — impossible with multiple
vector columns and hostile to text/edge/scalar locality.

**Decision — three internal ID layers:**
- `global_row_id: u64` — stable identity; edges & cross-file refs (D-001).
- `local_offset: u32` — row position; column chunks, MVCC, text postings, CSR source.
- `hnsw_node_id: u32` — BFS-reordered, internal to one HNSW section; drives
  `page_id = hnsw_node_id / nodes_per_page`.

HNSW slot is addressed by `hnsw_node_id`, stores its `local_offset` (forward map for
rerank/MVCC), and neighbor lists store `hnsw_node_id`s. The reverse map
(`local_offset -> hnsw_node_id`) is build-time only; not needed at read time.
