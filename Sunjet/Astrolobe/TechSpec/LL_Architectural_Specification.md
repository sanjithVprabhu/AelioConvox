

**LL**

An AI-Native Database

*Architectural Specification*

**Confidential – Project LL**

*Design specification capturing the full system architecture, locked decisions, deferred items, and the v1 → v2 → v3 roadmap.*

Prepared as a comprehensive architectural reference for the LL build

# **Executive Summary**

LL is a database designed from physical bits to deployment topology around a single thesis: an AI-native database must integrate relational, vector, full-text, and graph storage and querying into one storage substrate, one write-ahead log, one transaction model, one cost-based optimizer, and one consistent file format. Anything less is a federation of subsystems with application-level glue, which is the architectural failure mode every existing 'multi-modal' or 'AI-native' database has fallen into.

This document captures the complete design of LL produced through a deep architectural conversation. It is intended as the canonical reference for the build. Every major decision is recorded along with the rationale that produced it, the alternatives that were rejected, and the honest tradeoffs being accepted. Items deferred to later versions are explicitly flagged so the v1 scope is unambiguous.

### **What LL is**

A single-node, AI-native database built in Rust. Its physical record is a typed, versioned tuple whose columns can be scalars, vectors, full-text-indexed text, or typed graph edges, all landing in the same WAL entry and versioned together via MVCC. Its file format is a custom columnar format with embedded HNSW pages, FST \+ PFOR-Delta posting lists, and CSR graph adjacency, all per-file and all self-contained. Its query engine compiles SQL (with vector, text, and graph extensions) and a Python DSL into a unified logical plan optimized by a cost-based planner that estimates vector-search cardinality under predicates using per-file centroid distance distributions — a genuinely novel piece of optimizer work. Its execution engine is morsel-driven on Apache Arrow, built on top of DataFusion and extended with multi-modal operators. Its embedding lifecycle is owned by the database: **EMBED FROM** columns trigger system-managed generation, caching, regeneration on source updates, and dual-read migration when the user changes the embedding model.

### **What is genuinely novel**

Every individual technique used in LL has prior art somewhere. The novelty is the integration. To restate it directly: no database in production today has a single file format carrying HNSW pages, posting lists, and CSR edge tables alongside columnar scalar data, with a unified MVCC model and an optimizer that reasons about vector, text, and graph predicates in one cost model. Lance comes closest and has explicitly punted on text and graph. Lucene's inverted index is mature but has no columnar story, no vectors, no graph. DiskANN has the vector page discipline but is a single-purpose system. LL is the integrated artifact none of them are.

### **What is locked for v1**

The v1 scope is large but bounded. The storage layer (record, memtable, file format, compaction, buffer pool) is fully designed. The metadata layer (catalog, schema evolution, model registry) is complete. The query layer (SQL \+ DSL surfaces, logical and physical plan IR, cost model, optimizer) is specified. The execution engine (morsel-driven via DataFusion+Arrow) has all multi-modal operators defined. Transactions (Snapshot Isolation, optimistic concurrency) are settled. The embedding lifecycle (deferred-by-default, dual-read migration) is specified. The wire layer ships native gRPC plus partial Postgres wire compatibility. Observability, recovery and durability, operations, and security are designed.

### **What is deferred**

Distribution is explicitly deferred to v2 (read replicas \+ standby), v3 (sharded writes), and v4+ (multi-region). Application-level encryption-at-rest, OAuth/mTLS authentication, hash-chained audit logging, adaptive execution, and the native Python DSL plan-building API (as opposed to the v1 SQL-based wrapper) are also deferred. The v1 architecture is explicitly designed to preserve the primitives — stable row IDs, logical WAL records, independent file format, LSN-based MVCC — that make the distribution path feasible.

### **Build cost**

LL is hard. A realistic estimate is 2–3 years of focused work with a small, competent team, or 4–5 years solo. The file format alone is months of careful design and testing. The cost model is research-grade work. The query planner extensions to DataFusion are substantial. The embedding lifecycle is its own product subsystem. This is not a wrapper over existing infrastructure; it is a database in the literal sense of that word. The premium for doing the integrated work nobody has done is paid in engineering time. The reward is a system that is genuinely differentiated rather than another vector-database-plus-bolt-ons.

# **Document Overview**

This specification is organized into nineteen parts, working bottom-up from the physical record to deployment topology. The order is deliberate: each layer constrains the one above it and is constrained by the one below it, and reading bottom-up makes the constraint propagation visible. A reader who wants the executive view should read the Executive Summary and the Comparison with OriginChain v1 section. A reader who wants the architectural overview should additionally read the Architecture Principles section and the section introductions. A reader who wants implementation depth should read all sections in order.

### **Conventions used in this document**

**Locked decisions** appear in highlighted boxes at the end of each major section. These are commitments that the rest of the architecture depends on; changing them later is expensive.

**Deferred items** are explicitly named and assigned to a target version (v2, v3, etc.). The v1 scope is everything not so marked.

**Tradeoffs** are stated honestly. Every nontrivial design decision in a database involves real costs. Hiding those is how systems quietly fail when they meet production.

**Rejected alternatives** are described where the decision was nontrivial, so a future reader knows what was considered and why it was not chosen.

### **Sections**

* Part I — Background and Motivation: the OriginChain v1 lesson and the thesis of an AI-native database.

* Part II — Architecture Principles: the five principles that bind the rest of the design.

* Part III — The Physical Record: the atomic unit of storage.

* Part IV — The Memtable: the in-memory write path.

* Part V — The File Format: column chunks, embedded indexes, footer.

* Part VI — Compaction: how files evolve.

* Part VII — The Buffer Pool: page caching with heterogeneous page types.

* Part VIII — The Catalog: schema, metadata, model registry, schema evolution.

* Part IX — The Query Plan: SQL surface, DSL surface, logical and physical plan IR.

* Part X — The Cost Model and Optimizer: cardinality estimation across modalities, hybrid scoring.

* Part XI — The Execution Engine: morsel-driven parallelism, multi-modal operators.

* Part XII — Transactions and Isolation: MVCC semantics, conflict handling.

* Part XIII — The Embedding Lifecycle: generation, caching, migration with dual-read.

* Part XIV — The Wire Protocol and SDK: gRPC native, Postgres compatibility, client libraries.

* Part XV — Observability: EXPLAIN, telemetry, system tables, metrics, tracing.

* Part XVI — Recovery and Durability: WAL fsync discipline, checkpoints, crash recovery, backup.

* Part XVII — Operations: deployment shapes, configuration, upgrades, monitoring.

* Part XVIII — Security: authentication, authorization, encryption, audit, AI-native concerns.

* Part XIX — Distribution Roadmap: v2 read replicas, v3 sharding, v4+ multi-region.

* Part XX — Comparison with OriginChain v1: what changed structurally.

* Part XXI — Version Roadmap: precise v1, v2, v3, v4 scope and migration paths.

# **Part I — Background and Motivation**

## **The OriginChain v1 lesson**

LL exists because OriginChain v1 was, at the storage layer, a vector database with a hashmap of relationships. Functionally it was pgvector with extra metadata. Architecturally it was a federation of subsystems with application-level glue: a vector index, a separate relationship store, a separate metadata store, joined and merged at the application layer. The patents, the brand work, the GTM motion, the website, the architecture diagrams, the NVIDIA Inception co-branding all sat on top of a substrate that was, at the bytes-on-disk level, conventional. The product promise of an 'AI-native managed database with SQL, vector, full-text, graph, and natural language queries on a single managed endpoint' did not deliver at the layer that matters most: the storage substrate.

*The reason the federation pattern keeps losing is that everyone starts at the API layer and works downward, by which point the storage decisions are already locked in by whatever they grabbed (RocksDB, Lucene, hnswlib, Neo4j-ish). The interesting design space is bottom-up: what does a byte on disk look like when it has to serve all four access patterns, and what does that force everything above it to look like?*

The failure was not effort or talent; it was starting at the wrong layer. OriginChain v1 worked downward from the product surface. The integration was at the application layer because the storage substrate did not support integration. By the time the limitations became visible — inconsistent latency claims across product pages, conflicting graph algorithm counts, a vector latency story that depended on what was being indexed — the architecture was locked. The website inconsistencies were not marketing problems; they were symptoms of an architecture that could not make crisp performance claims because it was not crisp underneath.

## **Why a different approach is necessary**

LL inverts the design direction. Instead of asking 'how do we expose vectors, text, graph, and relational from a single endpoint,' it asks 'what does a record on disk look like when it must serve all four access patterns, and what does that force everything above it to be?' This is not a stylistic preference. It is the only way to produce a system that can push a hybrid predicate down into a single execution plan, that can guarantee read-your-writes across all four access patterns, that can rollback an embedding failure as part of a transaction, that can compact four index types together while reconciling MVCC. None of those properties are achievable when the substrate is federated.

Most systems that call themselves AI-native are vector databases with bolt-ons. Pinecone, Weaviate, Milvus, Qdrant are vector-first systems that have added text or metadata search as separate subsystems. Elasticsearch and Vespa are full-text systems with vector additions. Neo4j and TigerGraph are graph systems with optional vector and text. Lance is the closest existing artifact to LL's intent and has explicitly punted on text and graph. The market is wide open for the genuinely integrated system.

## **The thesis of an AI-native database**

*A database is AI-native when vectors, text, graph, and relational data share one storage engine, one transaction log, one query planner, and one cost model. Anything less is plumbing.*

Three properties follow from that thesis, and they are the hard part:

**A unified physical record.** A row in a documents table is not a row plus a vector hanging off in another system. It is a single record whose columns can be scalars, vectors, posting-list-indexable text, or graph endpoints. They land in the same WAL entry. They are versioned together under MVCC. A failed embed rolls back the row.

**A unified query plan.** A hybrid query compiles to one logical plan that gets reordered by a cost-based optimizer with cardinality estimates for vector search (recall versus selectivity), text search (posting list size), and graph traversal (out-degree distribution). Pushing a scalar predicate into the middle of a vector search must be a normal planner operation, not an application-layer trick.

**A unified storage format.** Arrow in memory, an extension of Parquet (or a custom columnar format) on disk, with extension types for HNSW pages, CSR-encoded adjacency, and FOR-compressed posting lists. One LSM tree, one compaction strategy that understands all four index types.

If those three are nailed, the rest — SDK, API, managed deployment — is normal infrastructure work. If they are not, the result is OriginChain v1 with better marketing.

# **Part II — Architecture Principles**

Five principles bind every decision in this document. They are stated here in advance because they are referenced repeatedly and because they justify a number of design choices that look unusual in isolation.

### **Principle 1: One substrate, one WAL, one MVCC model**

Vectors, text, graph, and relational data share storage. They live in the same files. They land in the same WAL entry. They version together via xmin/xmax. A failed write rolls back consistently across all four index types. This is the unification thesis stated as an engineering principle.

### **Principle 2: Read-your-writes across every access pattern**

The instant a transaction commits, every query type — point lookup, vector similarity, text match, graph traversal — sees its effects. There is no 'near real-time' visibility lag like Elasticsearch's translog model. The memtable carries in-memory analogs of all four index structures so recent data is queryable through every pattern. This is the correctness guarantee that distinguishes a database from a search engine.

### **Principle 3: First-class operators, not function calls**

Vector search, text matching, and graph traversal are first-class logical operators in the query plan, not function calls embedded in WHERE clauses. This is what allows the optimizer to reason about them — reorder them, push predicates through them, pick physical implementations based on cost. The standard pgvector approach of ORDER BY embedding \<-\> q LIMIT 10 with the vector distance as an opaque function defeats the optimizer; it cannot decide between pre-filter and post-filter strategies because it cannot see the operator structure.

### **Principle 4: Bottom-up design**

Storage decisions constrain everything above them. We design the physical record, the memtable, the file format, and the WAL first. Only then do we design the query plan, the execution engine, the SDK, and the deployment story. This is the discipline that prevents the OriginChain v1 failure mode of locking in storage limitations under a product-facing surface.

### **Principle 5: Single-node first, distribution as additive layers**

v1 ships a fully functional single-node engine. Distribution is added as v2 (read replicas), v3 (sharded writes), and v4+ (multi-region). The v1 architecture is designed to preserve the primitives — stable row IDs, logical WAL records, file independence, LSN-based MVCC — that make distribution feasible later. This is conservative scaling, deliberately chosen over a Spanner-style distributed-from-day-one architecture because the engineering cost of the latter is 5x higher and most users do not need it.

# **Part III — The Physical Record**

Every layer of LL is built on the physical record. Its definition determines what is possible above it. We design it first because it is the deepest design decision and the one that most constrains everything else.

## **The thesis: a typed versioned tuple that is also a graph node**

A physical record in LL is a **typed, versioned tuple with a stable identity**. Its columns can be scalars (the standard SQL types), vectors (with dimension and indexing semantics declared at schema time), text (with optional inverted-index semantics declared at schema time), or **typed references to other record identities** — edges. The stable identity doubles as a graph node ID. Indexes over the record store come in four flavors — B-tree on scalars, HNSW on vectors, posting list on text, adjacency on typed references — and all four are built from the same primitive: given this column's values, give me an efficient way to find records.

The collapse of category this enables is the central insight worth holding. The graph is not a separate subsystem; it is a view over the record store enabled by typed reference columns and edge indexes. Foreign keys are edges. Vector neighbors in the HNSW are edges with type vector\_neighbor. Temporal predecessors can be edges. Once you accept that every record has a stable identity that is also a graph node ID, the question of 'is this a row or a node?' stops being a schema-time decision the user has to get right; it becomes an access pattern the system supports.

## **The row IDs**

Each record has an internal row ID assigned at write time. Internal row IDs are a tuple (file\_id, local\_offset) for records in files, and (memtable\_id, local\_offset) for records still in the memtable. They are stable: once assigned, they never change, even as the record is updated (updates produce new versions with the same row ID). They are dense within a file: a file with N rows has row IDs 0 to N-1.

User-declared primary keys are separate from internal row IDs. PKs may be strings, composite, or updatable; row IDs are fixed 64-bit integers, never change, perfect for edge references and HNSW page references. When a foreign-key-style edge column references another row, what is stored is the target's internal row ID, not the target's PK. PKs are resolved to row IDs at insert time via the target table's PK index. This decoupling means edge storage is small and uniform regardless of the user's PK scheme.

## **MVCC fields**

Every record carries two system columns: xmin (the LSN at which this version was created) and xmax (the LSN at which it was superseded, or ∞ if still live). Reads at snapshot LSN T see a record version V iff V.xmin ≤ T \< V.xmax. These are stored as columns alongside the user's columns; compression handles them well because most records in a file have similar xmin values (they were written close in time) and most records have xmax \= ∞ (they have not been deleted).

## **The eager-relationship trap, explicitly rejected**

A tempting framing of the physical record is the 'human memory' metaphor: every record arrives at a point in time and automatically forms relationships with spatially-near (semantically similar) and temporally-near (co-occurring) records. This framing is rejected. Eager all-to-all relationship formation at write time is O(N) per insert against the entire dataset — at 10M records that is 10M similarity computations per write, which is a non-starter.

The discipline LL adopts instead: edges that the schema declares are eager (foreign keys, explicit graph edges); edges that come from indexes are lazy and approximate (HNSW neighbors, posting-list co-occurrence); edges that come from queries are computed on demand and optionally materialized as views. Each relationship type is paid for by the layer that benefits from it, not by every write.

## **The event-log framing, also rejected**

A second tempting framing is records as events in an append-only log — Datomic-style. This is rejected too. The right model is MVCC versioning of records through time, not events. Updates produce new versions; old versions are retained until GC; this is well-understood and correct. The append-only event log model has known costs (read amplification, query complexity, schema evolution pain) that we do not want to inherit.

## **The atomic write unit**

When the user runs INSERT INTO documents (id, title, body, embedding, author\_node) VALUES (...), a single record is created with all those column values populated. One row, multiple typed columns, one WAL entry. The HNSW page entry, the inverted-index posting, the edge index entry, and the row's columns are all derived from this single atomic write — they are not separate writes that need to be coordinated.

| Locked Decisions — Physical Record |
| :---- |
| A physical record is a typed versioned tuple with a stable internal row ID. Columns can be scalars, vectors, text, or typed references (edges). Internal row IDs are (file\_id, local\_offset) or (memtable\_id, local\_offset); 64-bit, never reused. User-declared primary keys are distinct from internal row IDs; PK resolution happens at write time. Edge columns store target internal row IDs, not target PKs. Every record carries xmin/xmax system columns for MVCC. The graph is an access pattern over indexed edge columns, not a separate subsystem. Edges are eager (schema-declared), lazy (index-derived), or computed (query-time) — never auto-formed. Versioning is MVCC, not event-sourcing. |

# **Part IV — The Memtable**

The memtable is the in-memory write absorber. Writes land here, become queryable immediately across all four access patterns, and eventually flush to immutable files. Most multi-modal databases get the memtable design wrong because they have a memtable only for scalars and bolt the vector and text parts on as eventual consistency. LL's memtable is the layer where read-your-writes across all four access patterns is delivered.

## **The principle: in-memory analogs of every disk index**

For every disk-grade index structure, the memtable carries an in-memory analog optimized for the memtable's size. The disk version is optimized for billions of rows with logarithmic or sublinear lookups. The memory version is optimized for \~100K rows where simpler structures (flat arrays, hash maps, small skiplists) are actually faster than their fancier counterparts because they fit in cache and have better constants. This is the same insight that makes LSM trees work in general; we apply it to all four index types.

## **Internal structure**

The memtable's heart is a row store keyed by row ID. It is a concurrent, lock-free skiplist where the key is the row ID and the value is the full record — all columns, including the vector, text, and edge columns. This skiplist is the source of truth for memtable data. Point lookups by ID hit it directly. Range scans by ID are sorted-order traversal. MVCC visibility is checked at this layer.

Skiplist is chosen over hashmap because we need range scans (for table scans). Skiplist is chosen over B-tree because lock-free skiplists handle concurrent writes far better than B-trees in memory — Java's ConcurrentSkipListMap and RocksDB's inline skiplist are the well-known reference implementations and scale to many writer threads.

Around the central skiplist sit four lightweight in-memory indexes:

**Secondary hash maps per indexed scalar column.** For point lookup by indexed columns (unique keys, foreign keys), a hash map mapping value to row ID. Cheap to update on each write.

**A flat vector array per vector column.** A contiguous float array of all vectors with their row IDs. SIMD dot-product brute-force search at query time. For a 256MB memtable holding 30K vectors at 1536 dim, a brute-force similarity computation against all 30K takes a few milliseconds — slower than HNSW but acceptable, and correct (100% recall). The flat array is the right structure at memtable scale; a tiny in-memory HNSW is optional and not in v1.

**An in-memory inverted index per text column.** As rows are written, tokens are extracted from text columns and a hashmap-of-tokens-to-row-IDs is updated. Small (because the memtable is small) and cheap to maintain.

**Bidirectional edge hash maps per edge column.** Forward (source row ID → target row IDs) and reverse (target row ID → source row IDs). Both updated on every edge write. Traversal queries hit these directly for recent edges.

## **Sizing: 256MB default**

The memtable size sets four things simultaneously: write throughput before stall, recovery time after crash, read amplification, and row group size at flush. These pull in different directions.

Production calibration: RocksDB defaults to 64MB. ClickHouse uses \~1GB in-memory parts before merging. Cassandra defaults to 25% of heap, typically 1–4GB. Lucene segments are typically formed from buffers of 16–128MB. Pinot uses 100–500MB. Most production systems land in 64MB–1GB; the right answer depends on workload.

LL defaults to 256MB. The reasoning is not 'the middle is safe.' It is a specific tradeoff matched to the AI-native workload: favor write predictability over read efficiency, recover from crash quickly, accept some compaction overhead. For an analytical workload we would push toward 1GB+; for pure OLTP, 64MB. AI-native sits in the middle, so the memtable does too.

Concretely for a 256MB memtable: with an average row of \~8.5KB (UUID id, \~100B metadata, \~2KB text body, 1536-dim float32 embedding, a handful of edges), the memtable holds \~30,000 rows. In-memory index overhead pushes the total in-memory footprint to \~300–400MB because the flat vector array alone is 30K × 6KB \= 180MB. Fine on a server, worth knowing for capacity planning.

## **Frozen memtables and flush**

The memtable size is configurable. Up to 4 frozen memtables can be in flight at once: at any moment there might be 1 active memtable (taking writes, up to 256MB) and up to 4 frozen memtables (full, waiting to flush, up to 1GB total). This gives burst absorption without making each individual memtable huge.

Flush triggers on whichever comes first: size threshold (256MB by default), row count threshold (100K rows so vector-heavy workloads do not flush at tiny row counts), or time threshold (5 minutes so low-write tables get flushed eventually). All thresholds configurable.

When a memtable fills, it is frozen (a new memtable starts taking writes) and the frozen one becomes immutable. Now expensive work can happen: convert the row skiplist to columnar layout, build a real HNSW from the flat vector array, build a real posting list with FOR compression from the inverted index, build CSR-compressed edge blocks from the edge hash maps. All of this happens in a background thread with no foreground latency impact.

The frozen memtable remains queryable using its in-memory structures until the flush completes and produces a row group file. Then the catalog atomically swaps: queries now hit the row group instead of the frozen memtable, and the frozen memtable can be discarded. There is no visibility gap. Data is queryable from commit through flush via in-memory structures, and continues to be queryable after flush via row group structures.

## **HNSW promotion at flush is rejected**

The in-memory flat vector array is discarded at flush time and a fresh HNSW is built in the row group. We do not attempt to 'promote' the flat array to disk. The reason: HNSW construction quality depends on ef\_construction and insertion order, and a high-quality HNSW for 30K vectors built once with ef\_construction=400 is better than an incrementally evolved one. Lucene takes the same position with its segments — built fresh from buffered docs, not by evolving a previous index. The memtable's in-memory structures are write-optimized; the row group's structures are read-optimized; flush is the conversion point.

| Locked Decisions — Memtable |
| :---- |
| Lock-free skiplist as row store keyed by row ID. Per-column secondary hash maps for indexed scalar lookups. Flat vector array with SIMD brute-force similarity for vector search. In-memory inverted index per text column. Bidirectional edge hash maps per edge column. 256MB default memtable size, configurable. Up to 4 frozen memtables in flight (writers never wait for flush). Flush triggers: size (256MB), row count (100K), or time (5 minutes), whichever first. At flush, in-memory structures are discarded and disk structures built fresh. |

# **Part V — The File Format**

The file format is the most consequential artifact LL produces. Every byte of every persisted row goes through it. It will outlive every other part of the codebase. We design it as an original artifact, drawing from Parquet (footer-with-metadata layout), Lance (vector-native pages), RocksDB SSTables (LSN-tagged structure), Lucene segments (embedded inverted indexes), and DiskANN (vector page discipline), but with the specific integration none of those have.

## **Why a custom format**

Parquet was designed for batch analytics — write once, read many. It has no concept of indexes embedded in files. It cannot store HNSW pages, posting lists, or graph adjacency natively. Stuffing binary blobs in Parquet columns is what 'Parquet \+ vector' systems do, and at that point the format is just a container; the database has to maintain external indexes that go stale relative to the Parquet files. That is the OriginChain v1 trap reframed at the storage level.

Lance is the closest existing format and was designed for ML training data lakes, not transactional databases. Its versioning is fragment-level, not record-level. It does not have built-in posting lists or graph adjacency. It does not have the LSN-tagged page metadata that LL's LSM recovery model needs. Extending Lance would require so much modification that we would be forking it anyway. Better to design our own with clean principles.

## **File-equals-row-group**

Each row group flushed from a memtable is one file on disk, immutable from the moment it is written. Compaction reads N such files and produces M bigger files. The catalog tracks which files exist and which LSN range each covers. This is the same model RocksDB uses for SSTables: one row group equals one file.

## **Top-level layout**

The structure of every LL file:

\[Magic number \+ format version: u32 \+ u16\]  
\[File header: LSN range, row count, schema fingerprint, creation timestamp\]  
\[Column chunks: one section per column, columnar data\]  
\[Embedded indexes: one section per index type relevant to this file\]  
\[Footer: offsets/sizes of everything above, statistics, bloom filters\]  
\[Footer length: u32\]  
\[Magic number: u32\]

Both magic numbers (start and end) bracket the file so corruption can be detected from either direction. The footer is at the end (the Parquet model) because at write time we do not know the offsets until we have written everything; writing the footer last is the only way to do it in a single pass. Reading the file is one small seek to the end, read the footer length, read the footer, now we know where everything is.

Endianness: little-endian everywhere (matches x86/ARM, no swap cost). Alignment: 8-byte alignment for vector data (SIMD needs it). Footer encoding: FlatBuffers (zero-copy read). Column data: raw bytes (no parsing overhead). Default compression: ZSTD level 3 (good ratio, fast decompress). LZ4 available as the low-CPU option.

## **Column chunks**

For each column in the schema, this section contains a column header (type, encoding, compression algorithm, null count, min/max, total bytes), the encoded column data (possibly dictionary-encoded, possibly compressed), optional dictionary if dictionary encoding is used, and per-page metadata (rows-per-page, page offsets within the chunk). Pages within a chunk are 8KB–1MB. Pages are the unit of decompression — independently decompressible so parallel decompression is possible.

### **Vector columns**

Stored as a contiguous float array (no per-value compression — we want SIMD-friendly raw access). Optionally scalar-quantized (FP32 → FP16 or INT8) with a quality tradeoff. The column header records the quantization scheme and dimensions.

### **Text columns**

Stored as compressed strings (ZSTD per page), with optional dictionary encoding for low-cardinality text. The text content lives here; the inverted index over the text lives in the embedded indexes section.

### **Edge columns**

Stored as variable-length lists of (target\_row\_id, edge\_property\_offset) pairs. Most rows have few edges; we use a compressed sparse representation. The column tells you 'for row N, edges start at byte offset X.' The actual graph traversal happens via the embedded edge index, but an inline copy lives here for cheap lookup of low-degree nodes.

### **Scalar columns**

Standard columnar encoding — dictionary, RLE, delta, FOR — whichever fits the data best. ZSTD compression at the page level by default.

## **HNSW page layout**

The vector index lives in its own section per vector column. The design borrows heavily from DiskANN's page discipline while keeping the HNSW algorithm.

### **DiskANN-style co-location**

The fundamental insight: store each node together with its neighbor list and its quantized vector, all on one page. One page read gets the vector for distance computation AND the neighbors to traverse. Without this, a search touching 1000 candidates becomes 2000+ disk reads.

### **Page structure**

Fixed-size 4KB pages. O(1) page lookup from node ID: page\_id \= node\_id / nodes\_per\_page. No B-tree to navigate. No fragmentation. Each page contains 4–32 nodes depending on vector dimensionality.

Per-node entry: node\_id (u32, the row offset within the file), layer\_count (u8), neighbor\_count\_per\_layer (u8 array), neighbors\_layer\_0 (u32 array, fixed-size 2M entries), neighbors for higher layers (variable), and the quantized vector (int8 array of dimension D).

### **Quantization with rerank**

HNSW pages contain int8-quantized vectors. Column chunks contain full-precision vectors. Search algorithm: traverse HNSW with quantized distances → get top-K candidates → fetch full-precision vectors from column chunk → rerank with exact distance → return top-K. Storage cost is \~1.25x (full precision in column chunk \+ quantized copy in HNSW page). Recall loss is \<2%. The 4x compression of int8 quantization means pages are smaller and more nodes fit per cache line.

### **Node-major layout**

Each node's data for all layers lives together. The dominant access pattern is 'read a node's layer-0 neighbors and its vector,' and node-major puts these together. The upper layers are small (layer 1 is \~1/M of layer 0, so \~6% with M=16), so having them inline does not bloat layer-0 pages much. The entry point is stored in the footer for instant access at search start.

### **BFS reordering at flush**

At flush time, when we build the HNSW for this file, the node IDs are reordered such that nearby-in-graph nodes get nearby IDs. Concretely: BFS from the highest-degree node, assign sequential IDs in BFS order. Cheap (linear in graph size), milliseconds for a memtable flush, seconds for large compactions. The locality benefit means a search traversal to a node's neighbor likely finds that neighbor on the same page or an adjacent page already in cache.

### **Deletion handling**

HNSW pages are immutable. Deletions are handled via MVCC at the rerank step: when we fetch full vectors, we also fetch xmin/xmax and filter. Search traversal still visits dead nodes; if too many accumulate, the file becomes slow and compaction rebuilds it without the dead nodes. Same pattern as Lucene segment merging.

## **Text section: FST \+ PFOR-Delta posting lists**

The inverted index for each text column. Twenty-five years of Lucene production search shaped this format; we take it almost wholesale and deviate only where we have a constraint or opportunity Lucene does not.

### **Term dictionary as FST**

Finite State Transducer for the term dictionary, using BurntSushi's fst Rust crate. Prefix and suffix sharing gives 5–10x compression versus storing terms in full. Cache-friendly traversal (contiguous byte array). Range and prefix queries fall out for free. Memory-mappable. FST outputs are u64 encoding the byte offset into the posting list region plus flags plus doc frequency.

### **Posting lists: PFOR-Delta with SIMD blocks**

Posting lists are sorted by doc ID, encoded with PFOR-Delta in 128-element blocks. Modern bit-packing decode using AVX2/AVX-512 instructions, 2–4x faster than non-SIMD. Doc IDs delta-encoded then bit-packed; frequencies bit-packed (skipped if all 1); positions stored optionally per column.

### **Skip lists for intersection**

Skip pointers every 128 entries enable logarithmic skipping during multi-term intersection. Essential for production search performance on AND/OR queries.

### **Score-augmented auxiliary lists (the deviation)**

For terms with posting lists larger than 10K entries, an auxiliary array stores the top-1000 doc IDs by impact score (BM25-contribution). Top-K text queries hit this auxiliary array first; if K confident answers come back, the main posting list is never touched. \~4KB per qualifying term. Simpler than WAND/BlockMaxWAND, and well-matched to AI/RAG workloads where 95% of text queries are simple top-K. WAND can be added later without disrupting this design.

### **Positions opt-in per column**

Phrase queries need positional data. Positional storage roughly doubles posting list size. Per-column opt-in: schema declares INDEX text (positions \= true) for columns that need phrase queries (body, descriptions), INDEX text (positions \= false) for columns that do not (tags, identifiers). Default true; users opt out when storage matters. Phrase queries against positionless columns fail loudly at planning time.

## **Edge section: forward and reverse CSR**

The graph index per edge column. This is the section with the least precedent to steal from — graph storage in a columnar/LSM context has essentially no production precedent — and we do original work here. The core data structure is CSR, the standard graph representation in numerical computing. It is read-optimal and mutation-impossible, which is fine because our files are immutable.

### **Cross-file edge problem**

Edges are pairs of (source\_row\_id, target\_row\_id). The source is in the file. The target can be in any file (including older ones). Forward traversal — 'what does S point to?' — is one file lookup. Reverse traversal — 'what points to T?' — could require scanning every file. We solve this with forward \+ reverse per file, accepting the 2x storage cost in exchange for predictable reverse-traversal performance.

### **Forward index: dense CSR**

Offsets array indexed by file-local source ID, edges array of target row IDs sorted by source then target. PFOR-Delta encoding throughout, SIMD decode. Property arrays parallel to the edges array, one per declared property — columnar properties (delta for timestamps, dictionary for labels, FOR for small ints, raw for floats). Properties that are not queried are not read.

### **Reverse index: sparse map**

Sorted array of target row IDs (one entry per distinct target referenced by this file's edges), parallel array of source pointer lists, fanout index over the targets every \~128 entries for binary search.

### **Bloom filter accelerator**

Per-file bloom filter over target row IDs. Reverse traversal of T queries every file's bloom filter first; only files where the bloom returns 'maybe' get their reverse index consulted. \~1KB per file. For a system with 1000 files where T appears in 5, we do 1000 bloom checks (cheap) and 5 real lookups.

### **Inline edges in row column**

The row's edge column contains an inline stub: count plus either inline target IDs (if count ≤ 8, the configurable threshold) or a pointer into the edge section's CSR. Low-degree nodes (most nodes in real graphs, since degree distributions are power-law) have edges fully inline — one page read for the row plus its outgoing edges. High-degree nodes pay one extra read for the CSR slice.

### **Compressed MVCC for edges**

Per-edge xmin/xmax would be 16 bytes per edge — at billion-edge scale, 16GB just for versioning. Instead: per-file base LSN in the section header, per-edge xmin stored as small delta (1–2 bytes, PFOR-Delta encoded), per-edge xmax sparse (only present for deleted edges). Typical compression: 1–3 bytes per edge for version metadata.

### **Schema-level reverse-indexing opt-out**

Some edge columns are never reverse-traversed. INDEX edge (col) WITH (reverse \= false) disables the reverse index for that column, saving the 2x storage cost. Default is reverse \= true.

## **The footer**

The file's table of contents. Every query, every scan, every traversal starts by reading the footer. Encoded as FlatBuffers for zero-copy read.

### **Contents**

Identity (format version, file UUID, creation timestamp, writer version). LSN range (writing range: min/max LSN of records in this file). Schema (full table schema as of when this file was written, with fingerprint hash). Statistics (row count, live row count, tombstone count). Section directory (typed per-section meta arrays for HNSW, text, edge, B-tree). Translation table (file-local row ID ↔ global row ID). Predicate pushdown metadata (per-column per-page zone maps, per-column file-level bloom filters). MVCC summary (visible\_lsn\_range: min\_xmin, max\_xmax across the file, for fast snapshot pruning). Shared compression dictionaries. Footer checksum (CRC32).

### **Zone maps for predicate pushdown**

Per column, per page: min, max, null count, distinct count estimate, total count. For scalar predicates like 'WHERE timestamp \> X', the planner reads zone maps and skips pages where max(timestamp) \< X. For vector columns, zone maps store per-page centroid and max-radius (vectors in this page are all within radius R of centroid C) — useful for approximate vector pruning.

### **Schema-in-footer for evolution**

The footer carries the full schema as of when the file was written. At read time, the executor projects between the file's schema and the current schema (column added later → null/default for old files; column dropped → ignored when reading; type widening → cast on read). This is the Iceberg/Delta Lake pattern. Schema fingerprint enables fast equality check — most files in a table share schema; we skip projection logic when fingerprints match.

### **Three-layer checksums**

Footer has its own CRC32. Each section has its own checksum (in section header). Each page has its own checksum (in page header). Three layers, each independently verifiable. On file open: validate footer. On section access: validate lazily. On page access: validate (CRC32 is fast, hardware-accelerated; microseconds per page).

| Locked Decisions — File Format |
| :---- |
| Custom columnar file format, one file per memtable flush, immutable. Magic \+ header → column chunks → embedded indexes → footer → footer length \+ magic. FlatBuffers footer for zero-copy read; little-endian everywhere; 8-byte alignment for vectors; ZSTD level 3 default. HNSW pages: 4KB fixed, node-major layout, int8 quantization with full-precision rerank, BFS reordering at flush. Text section: FST term dictionary (BurntSushi crate), PFOR-Delta posting lists in 128-element SIMD blocks, skip lists every 128 entries, score-augmented auxiliary lists for terms \> 10K postings, positions opt-in per column. Edge section: forward \+ reverse CSR per file, columnar properties, bloom filter for cross-file reverse traversal pruning, inline edges in row column for ≤ 8 targets, compressed per-edge MVCC. Footer carries schema, statistics, zone maps (centroid+radius for vectors, ranges for scalars), file-level bloom filters, translation table. Three-layer checksums (footer / section / page) using CRC32. Schema-in-footer enables online schema evolution (add column, drop column lazy, type widening). |

# **Part VI — Compaction**

Standard LSM compaction merges N sorted files into M sorted files, reconciling versions, dropping tombstones. RocksDB does this beautifully for a single key-value sort order. LL's compaction is harder in three specific ways: we have no single sort order (we have row IDs as the primary order, but per-file indexes — HNSW, FST, CSR — each have their own internal organization), the indexes have wildly different rebuild costs (HNSW rebuild dominates by orders of magnitude), and MVCC reconciliation has to work coherently across all four index types.

## **Strategy: tiered, not leveled**

Leveled compaction (LevelDB, RocksDB default) has lower read amplification but higher write amplification — each record gets rewritten \~10 times as it cascades. Tiered compaction (Cassandra, ScyllaDB) has higher read amplification but lower write amplification — \~3–4 rewrites per record over its lifetime.

HNSW rebuild cost is so expensive that write amplification dominates the cost model for LL. A vector having its HNSW representation rebuilt 10 times during leveled compaction is prohibitive. We use tiered. The read amplification cost is less painful than it sounds because our common queries (vector, text, graph) are file-parallel anyway — they query each file's index independently and merge. Touching 10–20 small HNSWs in parallel is often faster than touching one huge HNSW that does not fit in cache.

Concretely: size-tiered, 4 tiers, ratio 8x between tiers. Tier 0 holds files from memtable flushes (\~256MB). Tier 1 holds files merged from 4–8 tier-0 files (\~2GB). Tier 2 holds files merged from 4–8 tier-1 files (\~16GB). Tier 3 holds the largest files (\~128GB+). For 1TB of data, \~50–100 files total, mostly tier-2/tier-3, a few recent tier-0.

## **HNSW merge vs rebuild — the interesting question**

When compacting N files with HNSW indexes, two choices: rebuild from scratch (extract all vectors, insert into a fresh HNSW; best quality; expensive) or merge existing HNSWs (stitch graphs together; faster; 5–15% recall loss; quality gap grows with merges).

LL rule: rebuild HNSW from scratch at tier-1 → tier-2 and beyond. Merge HNSW for tier-0 → tier-1. The reasoning: small files are short-lived (will be rebuilt at next tier transition); big files are permanent (every query against them pays for whatever quality we baked in). Spending 30 seconds at compaction time to save 5ms × millions of queries × the file's lifetime is obviously right.

Same principle for BFS reordering: preserve order at small merges, run full BFS reordering at big-tier rebuilds. Concentrate expensive work where it matters.

## **Other indexes during compaction**

FST and posting lists: rebuilt from scratch at every compaction. FSTs are fast to build (\~milliseconds per 100K terms) and there is no quality degradation from rebuilding. CSR edge index: rebuilt from scratch (O(E), cheap; merging would complicate MVCC reconciliation across files). B-tree: rebuilt from scratch from sorted data (linear, trivial). Zone maps, bloom filters, score-augmented lists: all rebuilt from scratch.

HNSW is the only index where we have a merge-vs-rebuild policy. Everything else is always rebuild.

## **MVCC reconciliation**

During merge, for each row ID with multiple versions across input files: if the latest version has xmax \< global\_min\_active\_txn\_lsn, the row is fully dead — drop all versions. Otherwise keep the versions visible to live transactions. Reconciliation happens before index build, not after, because the output HNSW must contain only surviving rows.

## **Triggers**

Three triggers, in priority order:

**Tier-full trigger.** When tier N has accumulated K files (default K=4), schedule a merge to tier N+1. Primary trigger; bounds total file count.

**Tombstone-density trigger.** When a file's tombstone\_count / row\_count exceeds 30%, the file is dragging down search performance. Schedule a compaction with adjacent files in its tier. Fires regardless of tier fullness.

**Schema-evolution trigger.** When a column or index is added/dropped, files written before the change have stale layout. Normal tier-up compaction catches this opportunistically. For urgent cases (compliance, dropping a sensitive column), FORCE REWRITE compacts immediately.

Notably absent: time-based triggers. Files only need compaction when they are problematic; compacting old files just because they are old wastes I/O.

## **Concurrency**

2–4 background compaction threads (configurable). Compaction never blocks foreground operations. Reads of files being compacted go to the original files (still exist until completion). Writes go to the memtable. The only synchronization point is the atomic catalog swap at compaction completion: input files marked for deletion, output files made visible. Lock is held for microseconds; file deletion is deferred until no transaction needs the old files.

## **Back-pressure**

If write rate exceeds compaction rate, files accumulate at tier 0\. Standard answer: throttle. Target 8 files at tier 0\. At 16 files (2x target), start throttling new writes at 50% of normal rate. At 32 files (4x), throttle to 10%. At 64 files (8x), stall writes entirely. Operators see elevated write latency in metrics and know to add compaction throughput.

## **Cross-file optimizations at compaction time**

Since compaction reads N files and writes M files, we can do work that is not possible at flush time: global FST vocabulary (better prefix sharing in merged corpus), scaled HNSW parameters at larger files (higher M and ef\_construction for tier-2+ files), trained ZSTD dictionaries per column (better compression with larger corpus to learn from), proper sizing of bloom filters and zone maps based on observed distributions. Tier-0 files are fast-to-write but suboptimally compressed/indexed; compaction is where data is polished into read-optimal form.

| Locked Decisions — Compaction |
| :---- |
| Tiered compaction, 4 tiers, ratio 8x between tiers, target 4–8 files per tier. HNSW rebuild from scratch for tier-1 → tier-2 and beyond; merge allowed for tier-0 → tier-1. BFS reordering of HNSW node IDs at big-tier rebuilds; preserve order at small merges. All other indexes (FST, CSR, B-tree) rebuilt from scratch always. MVCC reconciliation before index build, not after. Three triggers: tier-full, tombstone-density (\>30%), schema-evolution opportunistic. No time-based triggers. 2–4 background compaction threads (configurable); never blocks foreground. Atomic catalog swap at compaction completion, deferred file deletion. Write throttling at 2x tier-0 target file count, stall at 8x. Cross-file optimizations at compaction: global FST vocabulary, scaled HNSW parameters, trained ZSTD dictionaries. |

# **Part VII — The Buffer Pool**

A buffer pool is an in-memory cache of disk pages. Every read goes through it. Misses are 1000x slower than hits (100μs SSD vs 100ns RAM), so policy and sizing determine the effective performance ceiling of the system. Most of the buffer pool design is well-trodden engineering. LL's specific contribution is heterogeneous page types with explicit pinning.

## **Direct I/O via io\_uring**

Decision: bypass the kernel page cache with O\_DIRECT (with io\_uring on Linux for batched async I/O). Reasoning: heterogeneous page types and structured access patterns (pinning footers, prioritizing HNSW entry points) defeat the kernel's uniform treatment. Predictable memory usage matters for operators — buffer\_pool\_size \= 32GB should mean exactly 32GB, not '32GB plus whatever the kernel decides to cache.'

Cost: we own alignment, batching, async submission. Real code that has to be correct under concurrent load. The Rust ecosystem (tokio-uring, glommio, monoio) is mature enough in 2026 that this is manageable. Fallback to standard async I/O on macOS/Windows.

## **Variable-size, byte-accounted entries**

Page sizes vary across our types: HNSW pages 4KB, column chunk pages 16KB default, posting list blocks 1–8KB, footers 100KB–5MB, translation tables KB to MB. Uniform-frame designs (Postgres-style 8KB everywhere) waste memory packing different sizes. We use variable-size entries with byte-level budget accounting. Each entry carries its size; the pool tracks current\_bytes against budget\_bytes; eviction decisions are by bytes, not entry count.

## **Eviction: W-TinyLFU with a pinned region**

Pure LRU is wrong for our workload — sequential column scans evict the hot working set (HNSW entry points, FST roots) and the next vector query pays a cold-cache cost. Frequency-aware policies handle this. W-TinyLFU (Einziger et al. 2017, used in Caffeine, foyer, moka) is the state of the art: a small LRU window catches scan misses while the main cache uses frequency-based admission. Hit rates within 1–2% of optimal, robust to scans, low overhead.

LL uses W-TinyLFU via the foyer Rust crate. 64 shards by default (page key hashes to a shard; each shard has its own lock, its own W-TinyLFU instance). Reference counting on pages prevents eviction during use.

## **Pinned region**

Some pages must never be evicted while their files are active. A small pinned region (1–5% of pool) holds explicitly-pinned pages: file footers (one per active file), FST root nodes per text-indexed column per file, HNSW entry points and top 1–2 layer pages, translation tables. Pinning is an explicit API. When a file is opened, the file manager pins its essential pages. When a file is dropped, pins release.

Footprint: \~200KB per file. For 1000 active files, 200MB pinned. For a 32GB pool, 0.6% — well within budget. Cheap insurance against statistical-policy edge cases.

## **Prefetching**

Direct I/O means we own read-ahead. Three patterns:

**Sequential column scans:** when reading page K, prefetch K+1, K+2, K+3 asynchronously. The query layer indicates 'this is a sequential scan' for prefetch eligibility.

**File-open warming:** when a file is first accessed, eagerly load footer, translation tables, FST roots, HNSW entry-point pages via one batched io\_uring submission. File-open becomes one async round-trip instead of many.

**HNSW: no automatic prefetch.** HNSW traversal is fundamentally random. The optimization here is the BFS reordering done at flush time, which provides spatial locality so adjacent pages tend to be on adjacent disk locations.

**Posting list scans:** one block ahead. Sufficient.

## **Other decisions**

Cache key: (file\_id, page\_id), packed into u128 for alignment, hashed via xxHash. File invalidation on file drop: generation-based, lazy (file IDs include a generation; mismatched generation means cache miss). Memory backing: mmap with MAP\_HUGETLB when available for hugepage support (reduces TLB misses for large pools); NUMA-awareness deferred to v2.

Sizing recommendation: for an N GB machine, reserve 4–8GB for OS, 1–4GB per concurrent heavy query for execution memory, allocate \~60–70% of remaining to buffer pool. On a 64GB box: \~36GB buffer pool. Configurable. We respect the budget strictly — no dynamic shrinking.

Cold-start warmup deferred to v2. After restart, performance ramps as the cache fills. v2 will support snapshotting the W-TinyLFU 'main cache' key list periodically and reloading at startup.

| Locked Decisions — Buffer Pool |
| :---- |
| Direct I/O via io\_uring on Linux (with fallback to standard async I/O on other platforms). Variable-size byte-accounted entries; reference counting to prevent eviction during use. W-TinyLFU policy via the foyer Rust crate, 64 shards by default. Two-tier structure: pinned region (footers, FST roots, HNSW entry points, translation tables) \+ dynamic region. Explicit pin/unpin API for file-resident essentials. Prefetching: sequential column scans (3 pages ahead), file-open warming (batched), one-block-ahead for posting lists, no automatic HNSW prefetch. Generation-based lazy file invalidation. Hugepages support when available; NUMA-awareness deferred to v2. Default pool size 60–70% of available RAM after OS and query-memory reservations, configurable. Cold-start warmup deferred to v2. |

# **Part VIII — The Catalog and Schema Layer**

The catalog is the connective tissue. Everything queries it to know what tables exist, what columns they have, what indexes are declared, which files belong to which table, what models are registered. Get this wrong and the rest of the system cannot reason coherently.

## **DDL: declare intent, not implementation**

The user declares what they want; the system figures out how. They do not specify HNSW page sizes or quantization schemes unless they want to tune. Concrete DDL example:

CREATE TABLE documents (  
    id              UUID PRIMARY KEY,  
    title           TEXT,  
    body            TEXT INDEX (positions \= true),  
    body\_embedding  VECTOR(1536) EMBED FROM body USING 'openai/text-embedding-3-small',  
    author          ROW REF users,  
    cites           ROW REF documents \[\],  
    created\_at      TIMESTAMP DEFAULT now(),

    INDEX btree (created\_at),  
    INDEX vector (body\_embedding) WITH (m \= 16, ef\_construction \= 200),  
    INDEX edge (author, cites) WITH (reverse \= true)  
);

Key declarations: **VECTOR(dim)** as a first-class type (not FLOAT\[\] or BYTEA); **EMBED FROM body USING model** makes the embedding column derived (system owns generation lifecycle); **ROW REF table** is a typed reference stored as an edge column; **INDEX vector ... WITH (...)** supports optional parameters with sensible defaults.

## **What the catalog tracks**

* Tables: name, schema fingerprint, table\_id, owner, options, timestamps.

* Columns: name, type, position, default value, constraints, EMBED FROM linkage, column\_id (never reused even when dropped).

* Indexes: type (btree/vector/text/edge), columns, parameters, status (building/active/dropping).

* Files: file\_id, table\_id, tier level, LSN range, file path, size, state (active/compacting/deleting).

* Schemas: full schema per table, versioned via schema\_version (never reused).

* Models: model\_id (system), name (user-assigned, versioned), provider, endpoint config, output dimension, status.

* Transactions: active transaction list, snapshot LSNs, min LSN for GC barrier.

* Statistics: per-table per-column for the optimizer (NDV, histograms, vector centroid distributions).

* Settings: system-wide and per-table configuration.

* Audit log: schema changes, model registrations, compaction events, security events.

* Users, roles, privileges, secrets.

## **Storage: redb-backed, in-memory cached**

The catalog is persisted in redb (a Rust-native embedded KV store with real ACID transactions). Battle-tested, well-maintained, MIT-licensed, single-file. ACID transactions matter for fine-grained updates like file registration during compaction. Bootstrap is trivial — open the redb file, read tables, done. We avoid the recursive bootstrapping headache of using LL's own engine for catalog.

The catalog is fully cached in memory after open. Reads almost never touch redb. The redb file is essentially write-through persistence. Reads hit redb only on cold start. For \~10K tables and 1M files, the catalog is \~100MB in memory — comfortable. For 10M files it would be \~1GB and we would reconsider. We are not there now.

## **Schema evolution**

**Add column.** Schema version increments; new column appended. Existing files don't have it (reads return NULL/default); new writes include it. Online and free. The Iceberg/Delta pattern.

**Drop column.** Column marked dropped in catalog; column\_id is not reused. Existing files still have the column data but it is invisible to queries. Lazy reclamation through compaction. FORCE REWRITE for urgent cases.

**Add index.** Status=building, background job scans existing files and builds the index per-file. We use file rewrite for v1 rather than side-files. Status=active when complete.

**Drop index.** Catalog update is immediate; files retain the index data until compaction.

**Change column type.** Widening (INT32 → INT64, FLOAT32 → FLOAT64) is online; readers cast on read. Narrowing or incompatible requires explicit FORCE.

**Change embedding model.** Triggers the dual-read migration described in the Embedding Lifecycle section.

## **The model registry**

Embedding models are first-class catalog objects. The DDL:

CREATE MODEL openai\_small AS HTTP\_API (  
    endpoint \= 'https://api.openai.com/v1/embeddings',  
    model \= 'text-embedding-3-small',  
    dimension \= 1536,  
    api\_key \= SECRET('openai\_key'),  
    max\_batch\_size \= 2048,  
    max\_concurrent\_requests \= 8,  
    rate\_limit\_tokens\_per\_minute \= 1000000  
);

The **SECRET()** indirection points to the secrets subsystem (see Security). The catalog stores the reference; the actual API key is resolved from a secrets backend at use time. This decoupling is important for security and operations.

Each registered model has a system-assigned model\_id (u64, never reused) and a user-assigned name. Multiple models can share a name with different versions; default resolution is to the latest version, but pinning to specific versions is supported. The model registry is the most AI-native feature of the catalog and the place where LL differentiates from generic databases — embedding is a first-class operation, not a layer above.

## **Internal row IDs vs PK-as-row-ID**

We separate them. Internal row IDs are fixed 64-bit integers, never change, perfect for edges and index references. User PKs are whatever the user declared — strings, UUIDs, composites — and may even be updated. Edge columns store internal row IDs; PKs are resolved to row IDs at insert time via the target table's PK index. Inserts that reference foreign rows have a small lookup cost. Acceptable.

## **Catalog API**

Reads (fast, in-memory):

catalog.get\_table(table\_name) \-\> Option\<TableMetadata\>  
catalog.get\_files\_for\_table(table\_id) \-\> Vec\<FileMetadata\>  
catalog.get\_indexes\_for\_table(table\_id) \-\> Vec\<IndexMetadata\>  
catalog.get\_statistics(table\_id, column\_id) \-\> ColumnStatistics

Writes (transactional via redb):

catalog.create\_table(stmt) \-\> Result\<TableMetadata\>  
catalog.alter\_table(stmt) \-\> Result\<()\>  
catalog.register\_file(file) \-\> Result\<()\>  
catalog.swap\_files(inputs, outputs) \-\> Result\<()\>  // for compaction

DDL auto-commits in v1 (each DDL statement is its own transaction). Transactional DDL is v2.

| Locked Decisions — Catalog |
| :---- |
| DDL declares intent (VECTOR types, EMBED FROM, ROW REF, INDEX with optional WITH params). Internal row IDs distinct from user PKs; edges store internal row IDs. Catalog persisted in redb, fully cached in memory after open. Tracks: tables, columns, indexes, files, schemas (versioned), models, transactions, statistics, settings, audit log, users, secrets. Schema evolution: ADD COLUMN free, DROP COLUMN lazy, ADD INDEX background-build via file rewrite, type widening allowed. column\_ids and schema\_versions never reused. Model registry as first-class catalog objects; secrets via SECRET() indirection. Per-file schema fingerprint for fast equality check. DDL auto-commits in v1; transactional DDL deferred to v2. API exposes typed Rust interface; redb backing hidden for future distribution support. |

# **Part IX — The Query Plan**

The query plan is where bottom-up meets top-down. Everything we have designed in the storage layer is wasted if the planner cannot exploit it. The query plan is what makes our storage layer matter.

## **WAL and one log: revisited**

Before query plan specifics, one architectural point bears restating because it is foundational. LL uses one WAL, not multiple per-index WALs. Multi-WAL approaches all hit the WAL mismatch problem: at crash time, different WALs may have crashed at different points, leaving the database structurally inconsistent (a row exists in the heap WAL but its vector entry was lost from the HNSW WAL). Solving this requires either 2PC across WALs (which negates the parallelism benefit and adds latency), idempotent index rebuilding from a primary log (which makes recovery impractically slow for HNSW), or hierarchical logging with a manifest log (which is just one WAL with extra steps). The right answer is one logical WAL with group commit for throughput; LSN as the universal synchronization primitive between the WAL and every derived structure. RocksDB-class throughput (1M+ commits/sec with proper group commit) on a single WAL exceeds any practical workload.

## **The surface language: SQL with extensions plus a Python DSL**

LL ships SQL with vector/text/graph extensions as the primary query language, plus a Python DSL that builds plans directly. SQL is mandatory for ecosystem access (BI tools, ORMs, analysts); the DSL is differentiation for AI engineers who want type-safe, composable, programmatic query construction.

### **SQL extensions**

\-- Vector similarity  
SELECT id, title FROM documents  
ORDER BY body\_embedding \<-\> EMBED('hedging strategies') LIMIT 10;

\-- Text matching  
SELECT id FROM documents WHERE body MATCH 'derivatives AND volatility';

\-- Graph traversal  
SELECT u.name FROM users u  
WHERE EXISTS (PATH (u) \-\[:follows\*1..3\]-\> (target) WHERE target.id \= $user\_id);

\-- Hybrid query  
SELECT d.id, d.title,  
       hybrid\_score(  
           text\_score(d.body MATCH 'derivatives'),  
           vector\_distance(d.body\_embedding \<-\> EMBED('hedging strategies'))  
       ) AS score  
FROM documents d  
WHERE d.created\_at \> now() \- INTERVAL '30 days'  
  AND EXISTS (PATH (d) \-\[:authored\_by\]-\> (a) WHERE a.id \= $author\_id)  
ORDER BY score DESC  
LIMIT 20;

Operators: **\<-\>** is distance (lower \= closer). **MATCH** is the text predicate. **PATH** is the graph traversal subexpression. **EMBED(text, model?)** computes embeddings at query time (cached). **hybrid\_score(...)** is a built-in fusion (RRF by default).

### **Python DSL**

v1 ships a Python client wrapping the SQL surface (generates SQL strings from method calls). v1.5+ adds a native DSL that builds plan objects directly:

docs \= client.table('documents')

result \= (  
    docs  
    .filter(docs.created\_at \> ll.now() \- ll.days(30))  
    .filter(docs.path('authored\_by').to(author\_id).exists())  
    .hybrid(  
        text=docs.body.match('derivatives'),  
        vector=docs.body\_embedding.near(ll.embed('hedging strategies')),  
    )  
    .select('id', 'title', score='score')  
    .sort('score', descending=True)  
    .limit(20)  
    .collect()  
)

The DSL gives composability (queries as Python objects, reusable functions), type safety (the IDE catches docs.titel typos), embedding flexibility (the user can pass numpy arrays from any model), and Pandas/Polars interop (results are DataFrames natively). It does not replace SQL; it complements it for AI-engineer use cases.

## **Logical plan IR**

A logical plan is an algebraic expression — a tree of operators describing what to compute, not how. Standard operators (Scan, Filter, Project, Join, Aggregate, Sort, Limit, Union) plus our novel additions:

**VectorSearch(input, column, query\_vector, k, distance\_metric):** first-class operator for k-nearest-neighbor search.

**TextMatch(input, column, query\_expression):** first-class operator for text predicate evaluation.

**PathTraversal(input, pattern, bindings):** graph traversal with path patterns.

**HybridRank(inputs, scoring\_function, k):** take multiple ranked input streams, fuse them, produce top-k.

Critically: these are operators, not functions buried in WHERE clauses. This is what lets the optimizer reason about them — reorder them, push predicates down, pick physical implementations based on cost. Compare to pgvector, where vector similarity is a function inside ORDER BY; the Postgres planner cannot reason about it as an operator and so cannot decide whether to filter-then-vector-search or vector-search-then-filter. By lifting these to first-class operators, LL's planner makes those decisions explicitly.

## **Physical plan IR**

For each logical operator, the physical planner picks a specific physical implementation. The key one for AI workloads:

**VectorSearch logical** becomes one of: **MultiFileVectorSearch** (parallel HNSW search across files, top-k merge), **FlatScan** (brute-force when input is small enough), or one of three **PredicatedVectorSearch** variants when there is a filter: **PreFiltered** (evaluate predicate first, brute-force on filtered candidates), **PostFiltered** (search returning more than k, then filter), or **IntegratedFilteredHNSW** (filtered traversal of the graph).

The three filtered-vector variants exist as distinct physical operators rather than as a strategy parameter on one operator because the cost model is cleaner and the decision points are explicit. The choice among them is the single most important optimization in vector databases under filters, and it is where most systems get it wrong.

**TextMatch logical** becomes **InvertedIndexLookup** (FST \+ posting list per file) or **ScanFilter** when no text index exists.

**PathTraversal logical** becomes **ForwardWalk** (use forward CSR), **ReverseWalk** (use reverse CSR with bloom filter pruning), or **BidirectionalSearch** (meet-in-the-middle).

## **Execution model in the plan**

Plans execute via morsel-driven parallelism. Plans are broken into pipelines at pipeline breakers (HashAggregate, Sort, HashJoin build side, top-K). Each pipeline runs morsels through a chain of operators on a worker thread; many workers operate on different morsels in parallel; pipelines coordinate at boundaries via shared state. Within a morsel, operators process data in vectorized Arrow RecordBatches. Two-tier parallelism: row-level morsels for relational operations, file-level parallelism for multi-modal sources (HNSW search per file, text search per file, graph traversal by frontier).

We build on DataFusion's execution framework. DataFusion provides solid execution scheduler, standard operators, Arrow-vectorized execution, and an optimizer framework we plug into. We extend it with multi-modal operators. This is similar to how InfluxDB IOx is built on DataFusion — the pattern works for non-trivial vertical databases.

| Locked Decisions — Query Plan |
| :---- |
| One unified WAL, group commit for throughput; LSN as global synchronization primitive. SQL with extensions (vector \<-\>, text MATCH, graph PATH, hybrid\_score) as primary surface. Python DSL on top — v1 wraps SQL; v1.5+ builds plans directly. Native gRPC \+ Postgres wire protocol on the wire (see SDK section). Logical plan IR: standard operators \+ first-class VectorSearch, TextMatch, PathTraversal, HybridRank. Physical plan IR: multiple implementations per logical operator; cost-based selection. Three physical operators for filtered vector search: Pre-filtered, Post-filtered, Integrated-filtered HNSW. Morsel-driven parallelism, two-tier (row-morsel \+ file-level for multi-modal sources). Vectorized columnar execution using Apache Arrow as the in-memory format. Built on DataFusion's execution framework; multi-modal operators are LL's contribution. Plans represented as Rust enums; each variant carries metadata for cost-based optimization. |

# **Part X — The Cost Model and Optimizer**

This is the part where the system becomes genuinely smart or stays mediocre. Most vector databases do not have a real cost model. They have hardcoded heuristics: always use HNSW, always filter first, always rerank top 100\. These work for the median query and fail catastrophically on the tail — particularly on filtered queries where the filter is very selective and HNSW-then-filter returns mostly empty results. Selectivity is the entire game; every interesting decision in the planner is 'which input is smaller, the vector candidates or the filter candidates?'

## **Currency: microseconds, calibrated to hardware**

Traditional databases use abstract cost units (page reads × cpu cost). LL uses estimated microseconds, calibrated periodically against actual hardware measurements. Reasoning: modern workloads mix I/O-heavy and CPU-heavy operators (HNSW is CPU-heavy, scan is I/O-heavy); abstract units obscure this. We need to compare across plans like 'scan \+ filter' versus 'HNSW search,' and these have very different cost profiles; same units make the comparison honest.

## **Cardinality estimation: the standard parts**

Standard scalar cardinality estimation is well-understood. Per column, we maintain row count, distinct value count (NDV), min/max values, null count, sampled equi-height histogram (\~256 buckets), and most-common-values list. These are computed during compaction (which already scans all data). Predicate selectivity uses the textbook formulas — histogram lookup for ranges, 1/NDV for equality, product-of-independent-selectivities for AND (acknowledged wrong but standard). We implement these via DataFusion's framework.

## **Vector cardinality: the centroid distance distribution**

Given vector\_distance(column, query) \< threshold, how many rows satisfy? This is genuinely hard because the distance distribution depends on the query vector's position in the embedding space, the clustering of the data, and the metric. A histogram of distances would depend on the query (unknown at compaction time).

LL's approach: at compaction time, for each file, compute a distance distribution sample. Pick 100 representative query vectors (sampled cluster centroids of the file's vectors). For each, compute the distribution of distances to all vectors in the file. Store the resulting distribution per centroid. At query time, given a query vector, find the closest sampled centroid; use that centroid's distance distribution as the estimate. Selectivity at threshold T is then 'fraction of distances below T' from the closest centroid's distribution. Approximate but principled. Closer query to a sampled centroid → more accurate. Storage cost: \~100KB per file. Planning cost: \~100μs.

To my knowledge, no production vector database does this. They either use fixed selectivity estimates (assume 1% of rows match any vector query) or skip cardinality estimation entirely. Doing it properly is a real differentiator.

## **Predicated vector cardinality: the critical case**

ORDER BY embedding \<-\> q LIMIT 10 WHERE created\_at \> '2024-01-01' is the canonical hybrid case. We need two estimates: predicate selectivity (fraction of all rows matching the filter, via standard scalar estimation), and the conditional probability that a top-K candidate satisfies the predicate. Under independence (acknowledged wrong in correlated cases but standard), the predicate selectivity in top-K equals the predicate selectivity in the full data.

Then the operator decision: with predicate selectivity 1%, we need to search for \~K/0.01 \= 1000 candidates to find K survivors. PreFilteredVectorSearch cost is filter\_cost \+ brute\_force(0.01 × N). PostFiltered cost is HNSW\_cost(1000) \+ filter\_cost. IntegratedFilteredHNSW cost is HNSW\_cost × adjustment\_factor(selectivity) where adjustment is empirically 1.5x–5x for medium selectivity, prohibitive for very low. The optimizer compares these and picks. Sweet spot for Integrated is roughly 1% \< selectivity \< 50%; PreFiltered wins below; PostFiltered wins above.

## **Text cardinality**

Easier than vector — we have the FST and the posting list per file. Posting list length \= number of documents containing the term, known exactly via FST output. AND queries: minimum of individual cardinalities (lower bound) or product (point estimate under independence). For top-K text queries, the score-augmented auxiliary lists give us per-file top-K candidates directly; cardinality is K.

## **Graph cardinality**

Hardest of all. Real graphs are power-law (degree distribution is heavy-tailed; average is meaningless). High-degree celebrity nodes blow up cardinality estimates. We maintain out-degree distribution histograms per edge column rather than average × count. For multi-hop traversals we use multiplicative estimates with degradation factors. For traversals deeper than 3 hops, the planner falls back to conservative defaults and surfaces low confidence in EXPLAIN ('estimated cardinality: 10000 (confidence: low — deep traversal)').

## **Hybrid query cardinality**

A query combining vector \+ text \+ graph \+ scalar predicates has multiple anchors. The optimizer estimates cardinality of each anchor, decides the order of evaluation, drives from the most selective anchor first. The non-obvious case: two predicates with similar cardinality but very different evaluation costs (text might give 5000 candidates at 1ms via posting list scan; date might give 1000 candidates at 10ms via full column scan). The time-based cost model handles this — we compare microseconds to microseconds, not abstract units.

## **Operator cost formulas**

Each physical operator has a cost formula given input cardinality. Some examples:

**MultiFileVectorSearch:** sum over files of (hnsw\_search\_cost \+ page\_fetch\_cost) \+ merge \+ rerank \+ MVCC filter. HNSW search cost \= ef\_search × distance\_cost × log(N) for descent plus ef\_search × distance\_cost for layer-0 beam. Distance cost \= dim × 4ns for int8 SIMD. For 30K-vector files at 1536-dim with int8 HNSW: \~50μs per file, \~10μs page fetch warm, \~100μs rerank. Total \~200μs per file. Across 100 files in parallel: \~5–10ms wall-clock.

**PreFilteredVectorSearch:** scalar\_filter\_cost \+ brute\_force(filtered\_rows × dim × 4ns) \+ top\_k\_selection. For 10K filtered rows at 1536-dim: \~60ms. For 1000 rows: \~6ms. Crossover with HNSW: \~1500 filtered rows.

Constants like distance\_cost\_per\_dim\_per\_ns and page\_fetch\_cost vary with hardware. LL calibrates: on system startup and periodically thereafter, run microbenchmarks (distance ops on synthetic vectors, page loads from cache vs disk, HNSW traversals, B-tree lookups, hash joins). Measured constants stored in the catalog; cost model reads them at planning time. ClickHouse and Snowflake auto-calibrate; we follow.

## **Optimizer architecture: rule-based \+ cost-based**

Standard hybrid architecture. Rewrite phase (rule-based): apply always-good transformations — predicate pushdown, constant folding, projection pushdown, common subexpression elimination, join reordering for obvious cases. Physical planning (cost-based): for each logical operator, enumerate candidate physical implementations; for each plan, compute cost; pick cheapest. Post-optimization: cleanup, parallelization decisions, batch size tuning.

## **LL-specific rules**

Predicate pushdown into vector search (Filter(VectorSearch) → PredicatedVectorSearch with cost-based variant choice). Predicate pushdown into text search (same pattern). Hybrid query decomposition (multi-modal query decomposed into per-modality sub-plans joined by ScoreFusion; order and parallelism are cost-driven). Graph traversal direction (forward vs reverse walk chosen by which constraint is more selective). File pruning (zone maps and bloom filters in footers skip files entirely).

## **Plan enumeration bounds**

CBO has a combinatorial problem. LL uses exhaustive enumeration for small queries (under 10 operators), DP-based for larger ones. Optimizer time is capped at 10ms hard limit for non-prepared queries; falls back to heuristic plans if exhausted. For prepared statements, plan once and cache.

## **Hybrid scoring fusion**

When a query combines vector, text, and scalar signals, how do we fuse? LL provides RRF (Reciprocal Rank Fusion) as the default — parameter-free, robust, used by Elastic and Vespa. Weighted linear combination as the customizable option. Plug-in interface for custom fusion (user-registered Rust function or SQL expression). Learned ranking deferred to v2.

Early termination optimization for ORDER BY hybrid\_score LIMIT K: each input stream produces top-K (with K'\>K=3K by default to avoid edge cases); union the candidates; compute full hybrid score only for the union; sort and return top K. This avoids computing scores for the long tail. Mathematically valid for RRF and most monotonic fusion functions.

| Locked Decisions — Cost Model and Optimizer |
| :---- |
| Cost \= estimated microseconds, calibrated periodically against real measurements. Standard scalar cardinality (NDV, histograms, MCV lists) computed at compaction. Vector cardinality via per-file centroid distance distributions (\~100 centroids × distance histogram per file). Predicated vector cardinality via independence assumption (with explicit acknowledgment of correlation cases). Text cardinality exact from FST \+ posting list lengths. Graph cardinality via out-degree histograms; deep traversal capped at low confidence. Hybrid query cardinality via per-modality estimates with most-selective-first ordering. Operator cost formulas per physical operator; calibrated constants stored in catalog. Optimizer: rule-based rewrite \+ cost-based physical planning \+ post-optimization. Novel rules: predicate pushdown into vector/text/graph, hybrid decomposition, traversal direction selection, file pruning. Plan enumeration: exhaustive (\<10 ops), DP-based (larger); optimizer time capped at 10ms. RRF as default hybrid fusion; weighted linear and custom plug-in supported. Early termination for ORDER BY hybrid\_score LIMIT K via per-stream top-K with K'=3K union. |

# **Part XI — The Execution Engine**

Most of the execution engine is well-trodden ground; we are not innovating for innovation's sake. What is specific to LL is the two-tier parallelism (row-morsel for relational ops, file-level for multi-modal sources) cleanly fitting under one scheduler.

## **Morsel-driven, vectorized, Arrow-backed**

The modern OLAP consensus (DuckDB, Velox, HyPer, ClickHouse). Data is divided into morsels (small chunks, \~100K rows or \~1MB) at the source. Each morsel flows through a pipeline of operators on a single worker thread. Many workers operate in parallel. Pipelines break at pipeline breakers (operators needing all input before producing output: HashAggregate, Sort, HashJoin build side, top-K). Within a morsel, operators process data in vectorized Arrow RecordBatches — columnar, SIMD-friendly, standard.

This composes naturally with backpressure: a slow consumer's input queue fills; upstream pipelines block on queue insertion. With parallelism: spawn N worker threads, each grabs morsels.

## **Pipeline structure**

A physical plan is broken into pipelines. Example for Scan → Filter → Project → HashAggregate: pipeline 1 is Scan → Filter → Project → InsertIntoAggregateHash (parallelized; workers grab scan ranges, run through, insert into shared aggregate hash table). Pipeline 2 is ReadAggregateHash → ProduceResults (starts after pipeline 1 completes). Pipeline breakers: HashAggregate, Sort, HashJoin (build side), HashSetJoin, top-K. Non-breakers: Filter, Project, MergeJoin (after sort), Limit in some cases.

## **Multi-modal sources as parallelized operators**

MultiFileVectorSearch runs as one pipeline source. Internal parallelism is at the file level — spawn N parallel HNSW searches (one per file or per file group). Each produces top-k candidates for its file. A merge operator combines into global top-k. The top-k flows downstream as a morsel. This is a different parallelism unit from row-level morsels; the scheduler handles both — file-level work units are morsels (with one or few rows each) once they leave the source.

Similarly: MultiFileTextSearch is per-file FST \+ posting list operations in parallel, then merge. Graph traversal is by frontier — BFS expansion of N frontier nodes in parallel; each expansion produces target nodes; new frontier is the union.

## **Memory management**

Each query has a memory budget (default 1GB; configurable; system enforces total across concurrent queries). Operators allocate from the query's budget via a tracked allocator. Spillable operators (HashAggregate, Sort, HashJoin) partition state into chunks; under memory pressure, oldest/largest chunks spill to temporary disk files. Non-spillable operators (top-K, filter, project) have bounded memory by construction.

For multi-modal operators: HNSW search has bounded memory (candidate set, size ef\_search). Text search posting list scans are streaming. Graph BFS frontier can grow; we cap depth and frontier size, fail explicitly when limits exceed. Global admission control queues new queries when total query memory would exceed system memory.

## **Operator state machines**

Each operator implements: init (prepare buffers, open child operators, look up indexes), process (handle incoming morsels), finalize (emit pending output, top-K materialization), cleanup (release resources). Stateful operators store state internally. State is partitioned for parallel access; workers have their own partitions; synchronization at finalize.

## **Cancellation and timeouts**

Each query has a CancelToken (atomic bool). Operators check it periodically — once per morsel, once per HNSW layer descent, once per text posting block. On cancellation, operators short-circuit, return immediately. The query coordinator collects the cancellation, releases resources, returns 'canceled' to the client. Timeouts are implemented as cancellation: a timer thread sets the token after the deadline.

## **Concurrency**

Multiple queries run concurrently. Each has its own memory budget, cancel token, worker thread pool reference (drawn from global pool), buffer pool reference (shared, read-only), catalog reference (shared, read-only after open). Worker threads are global; queries request workers up to their parallelism limit. Total parallelism bounded by max\_workers (default 2x cores).

LL uses tokio for I/O coordination (page fetches, async waits) and rayon for CPU-bound work (distance computations, hash table inserts). Hybrid is more code but the right answer.

## **Adaptive execution deferred**

Modern engines (Spark, Snowflake) adapt plans at runtime when actual cardinalities differ from estimates. Genuinely useful but complex. v2 work. For v1, we get the cost model close enough that we do not catastrophically mispick. Observability surfaces misestimates so users can intervene manually.

| Locked Decisions — Execution Engine |
| :---- |
| Morsel-driven parallelism, morsels as Arrow RecordBatches (\~100K rows default, configurable). Pipeline structure with pipeline breakers at HashAggregate, Sort, HashJoin (build), top-K. Two-tier parallelism: row-morsel for relational ops, file-level for multi-modal sources. DataFusion as the framework, extended with our multi-modal operators. Tokio for I/O coordination, rayon for CPU-bound work. Per-query memory budgets with operator-level tracking; spillable operators support disk spill. Operator state machine interface (init/process/finalize/cleanup). Cancel tokens for query cancellation and timeouts. Streaming results (no buffering by default). Multi-modal operators with internal parallelism: per-file HNSW, per-file text, frontier-parallel graph, priority-queue score fusion. MVCC visibility filtering integrated into source operators. No adaptive execution in v1; deferred to v2. |

# **Part XII — Transactions and Isolation**

Throughout the design we have assumed MVCC via xmin/xmax, snapshot reads at LSN T, and one unified WAL. This section pins down the precise semantics — which isolation level, what happens on conflicts, what guarantees we make. Get this wrong and the system is subtly broken in ways users do not discover until they have a corruption incident.

## **Isolation level: Snapshot Isolation**

LL's primary isolation level is Snapshot Isolation (SI). What Postgres calls 'Repeatable Read' though it's actually SI. Used by Oracle, SQL Server, MySQL, and most modern databases. We get it naturally from MVCC. Strong consistency for the common case (read-your-writes within a transaction, consistent reads within a transaction), no read locks (reads never block writes; writes never block reads), composes cleanly with our LSN-based architecture.

Read Committed is offered as a session option — each statement gets a fresh snapshot. Default isolation is Read Committed (matches Postgres default; familiar to most users). Serializable is not offered in v1 (true serializability requires either pessimistic locking that kills throughput, or Serializable Snapshot Isolation that adds complexity; deferred to v2). Read Uncommitted is not offered (provides no value, forecloses optimization opportunities).

## **The write path**

A write transaction's lifecycle:

* BEGIN: client requests transaction. System assigns txn\_id (monotonic counter). Transaction is 'active.' Snapshot LSN not yet assigned.

* First read (if any): snapshot LSN \= current LSN. Fixed for transaction lifetime under SI. Reset per statement under Read Committed.

* Writes: each write produces WAL records with provisional xmin \= txn\_id (not yet a real LSN). Writes accumulated in per-transaction buffer plus speculative entries in active memtable.

* COMMIT: system assigns commit\_lsn (sequential, atomic with WAL write). Appends COMMIT record to WAL with txn\_id and commit\_lsn. Fsync (with group commit). Replace provisional xmin \= txn\_id with real xmin \= commit\_lsn in memtable (eager resolution). Client sees success.

* ABORT or crash before commit: append ABORT record to WAL (or omit COMMIT). Provisional writes in memtable marked dead. On recovery, transactions without COMMIT records are aborted.

## **Commit log and eager resolution**

Each transaction's commit status lives in an in-memory commit log: txn\_id → commit\_lsn or aborted. Reading a row with xmin \= txn\_id checks the commit log: if committed, replace xmin with commit\_lsn and apply visibility check against snapshot; if aborted, row is invisible; if still active, invisible to other readers.

Eager resolution: at commit, we walk through this transaction's writes in the memtable and replace xmin \= txn\_id with xmin \= commit\_lsn. After commit, readers see resolved xmin and do not need commit-log lookup. Eager (vs lazy hint bits) trades small commit-time work for fast reads on hot data — right for our read-heavy workload.

## **Read path**

For each row produced by a source operator (Scan, HNSW, text search, graph traversal), apply visibility: visible if xmin ≤ snapshot\_lsn AND (xmax \> snapshot\_lsn OR xmax \= ∞). Also visible if row was written by current transaction (xmin \== my\_txn\_id) — read-your-writes. Invisible rows drop from results.

For source operators that internally index data (HNSW, FST), visibility happens after the index returns candidates. The index might return invisible rows; we filter at rerank time (HNSW) or after posting list intersection (text).

## **Write conflicts: optimistic concurrency**

Under SI, the canonical conflict is first-committer-wins for concurrent updates to the same row. Transaction A reads row R at version V; B also reads R at V; both compute new values; both try to update R. First to commit succeeds; second detects the conflict (R has changed since my snapshot) and aborts with 'serialization failure.' Client must retry.

Implementation: at commit time, for each row in the transaction's write set, verify the row's current xmin ≤ my snapshot LSN. If any fails, abort. Brief sharded row locks during this check. PK uniqueness enforced at commit (not insert). DELETE-DELETE: first wins, second aborts. INSERT with same PK: PK uniqueness check catches it at commit.

Write skew is the unavoidable SI anomaly we accept in v1. Users who need stricter guarantees use explicit locking (SELECT ... FOR UPDATE) or wait for v2's Serializable Snapshot Isolation.

## **Long-running transactions**

The min snapshot LSN across active transactions is the GC barrier — compaction cannot drop versions newer than this. A long-running transaction holds the GC barrier back. Mitigation: per-session max transaction duration (default 1 hour, configurable). Beyond max, transaction is auto-aborted with 'snapshot too old.' Background watcher checks active transaction durations every minute. Standard Oracle/Postgres pattern.

## **Crash recovery**

On restart: open the catalog, find the most recent CHECKPOINT record in WAL, scan from there forward to identify all transactions and their final status (COMMIT, ABORT, or active \= treated as aborted), replay WAL records into a fresh memtable applying only committed writes. Memtable rebuilds in seconds for typical WAL sizes (1GB → 5–10 seconds; 10GB → \~1 minute).

## **Transactions and embedding generation: the novel piece**

When a row is inserted with an EMBED FROM column, the embedding must be computed. When relative to the transaction?

**Deferred (default in v1).** Row commits without embedding. Embedding column is NULL and embed\_status='pending'. Background worker generates later. Row is queryable for scalar columns immediately; vector queries skip rows without embeddings until populated. Embedding generation is outside the user's transaction; commits are fast.

**Inline (opt-in per table).** WITH (embedding\_mode \= 'inline') makes embedding generation synchronous with commit. Transaction blocks on embedding API call (50–500ms). Failure makes INSERT fail. Right for low-volume, high-importance, read-immediately workloads.

## **DDL and transactions**

DDL auto-commits in v1. Each DDL statement is its own transaction. Schema changes have cross-cutting effects (compilation cache invalidation, etc.) that are easier to reason about without deferral. Background index builds cannot be transactional anyway. Transactional DDL is v2.

## **Connection model**

A client connection holds a session. A session holds transaction state. If a connection drops mid-transaction, the transaction is aborted by the server. Idle transactions tracked; sessions idle in transaction for \> N minutes get their transactions auto-aborted to prevent GC barrier hold.

| Locked Decisions — Transactions and Isolation |
| :---- |
| Snapshot Isolation as primary isolation level; Read Committed as session option; Read Committed as default. No Serializable in v1 (SSI deferred to v2). LSN-based snapshots; commit\_lsn at commit; in-memory commit log for active transactions. Eager xmin resolution at commit (write set walked, txn\_id replaced with commit\_lsn). Optimistic concurrency: first-committer-wins on row conflicts; second commit fails with serialization error. Write set tracked per transaction; commit-time conflict check with sharded row locks. PK uniqueness enforced at commit. Read-your-writes guaranteed within transaction. Long-running transaction protection: configurable max duration (default 1hr); auto-abort. Auto-commit DDL in v1; transactional DDL in v2. Deferred embedding generation as default; inline mode as opt-in per table. Pending-embedding rows commit with NULL embedding; background worker populates. Connection-bound sessions; transaction abort on connection drop. |

# **Part XIII — The Embedding Lifecycle**

This subsystem makes LL actually AI-native rather than 'database with vector column.' The user declares EMBED FROM and the system owns the rest: generation, regeneration on source updates, storage, querying, model migration, error handling, batching. That is a real promise — most vector databases do not make it. They make users generate embeddings client-side and insert them as data. LL says: you tell us the relationship, we maintain it.

## **The generation pipeline**

In deferred mode (the default):

* INSERT commits with embedding column \= NULL, embed\_status \= 'pending', embed\_model\_id \= current model.

* Row enters memtable; WAL has the insert.

* An embedding job is enqueued in the embedding job queue.

* Background embedding worker dequeues jobs in batches.

* Worker calls the embedding model (HTTP API, local ONNX, whatever).

* Worker writes back: UPDATE the row to set embedding \= result, embed\_status \= 'ready'.

* The update is a new version; old null-embedding version superseded via MVCC.

* Vector queries now see the embedding.

## **Job queue as system table**

The embedding queue is durable. We use a system table:

\_system.embedding\_jobs (  
    job\_id UUID PRIMARY KEY,  
    row\_id BIGINT, table\_id BIGINT, column\_id INT,  
    source\_column\_id INT, source\_value\_hash BIGINT,  
    model\_id BIGINT, status TEXT,  
    retry\_count INT, last\_error TEXT,  
    enqueued\_at TIMESTAMP, started\_at TIMESTAMP, completed\_at TIMESTAMP  
)

Workers dequeue with SELECT ... WHERE status='pending' ORDER BY enqueued\_at LIMIT batch\_size FOR UPDATE SKIP LOCKED. Standard work-queue pattern. Self-hosting is elegant — LL's own engine manages the queue. WAL gives durability. MVCC gives correctness under concurrent workers.

## **Batching**

Embedding generation is dominated by per-call overhead, not per-token compute. Calling OpenAI's API for 1 text vs 100 texts is almost the same wall-clock time. Workers batch aggressively: wake up, dequeue up to N pending jobs (default 128), group by model\_id (different models need separate calls), call each model with its batch, UPDATE corresponding rows with results, mark jobs done. Batch grouping by model is essential — we don't want one API call per model per worker iteration.

## **Worker concurrency per model**

API-based models: high concurrency (workers are I/O bound). Default 4–8 workers per model. Limited by API rate limits, tracked and respected via token bucket. Local GPU models: limited by GPU memory (typically 1–2 workers per GPU per model). Local CPU models: limited by cores (num\_cores / model\_thread\_count workers).

Rate limit enforcement: a token bucket per model. Workers consume tokens before making requests. If empty, workers wait. Prevents API rate limit violations and the cascade failures they cause.

## **The embedding cache**

The most important optimization in the embedding subsystem. Embedding the same text twice is wasteful. We maintain a cache keyed by (model\_id, source\_text\_hash). Per-model storage (one cache table per model to handle dimensional heterogeneity). Hash via BLAKE3 or xxh3 truncated to 64 bits.

Cache flow at write time: worker about to embed text T with model M computes hash(T), looks up the cache; on hit, uses cached vector and skips API call; on miss, calls API, stores result, returns. Cache flow at query time: when a user writes EMBED('text', 'model') in a query, the system computes the same hash and looks up the cache — repeated queries (common in RAG) hit the cache and skip API calls entirely.

Cache size: configurable max per model (default 10GB). LRU eviction based on last\_used\_at.

## **Error handling**

**Transient errors**  (rate limit, network timeout, 503): retry with exponential backoff. 5 retries by default with 1s, 2s, 4s, 8s, 16s delays.

**Persistent errors** (4xx that aren't rate limits, model deprecated, auth failure): don't retry. Mark job failed. Surface to ops.

**Hard errors** (source text too long, dimension mismatch): mark failed. Clear error message.

Failed jobs: status='failed', last\_error populated. Stay in queue for inspection. Operators can re-enqueue (reset to pending), manually fix, or delete. Failure metrics surfaced.

## **Application-side view**

On embedding failure, the row exists (scalar columns work). Vector queries miss this row. That is the contract: embedding failure does not fail the insert. Applications that need notification: optional per-table embedding\_failure\_webhook posts to a URL on each failed embedding (best-effort, not retried).

## **Inline embedding mode**

For users who need embedding-at-commit-time: WITH (embedding\_mode \= 'inline'). Insert blocks until embedding generated. Failure fails the insert. Transaction commits with embedding populated. No pending state. Right for agentic workflows where an agent inserts and immediately queries similar docs. Tradeoff: inserts are slow (50–500ms), throughput bounded by embedding throughput.

## **Model migration: the dual-read story**

A user has been embedding with text-embedding-3-small (1536 dim). They want to migrate to text-embedding-3-large (3072 dim) for better quality, or to a local model for cost, or to a fine-tune. The migration must not lose query availability, not require downtime, handle dimension change, be reversible, and be observable.

DDL:

ALTER COLUMN body\_embedding ADD MIGRATION TO MODEL 'openai/text-embedding-3-large';  
\-- migration runs in background

ALTER COLUMN body\_embedding COMPLETE MIGRATION;  \-- or ABORT MIGRATION

### **Phases**

**Phase 1 — Setup:** schema lists two active models for the column. Storage gets two concrete embedding columns (body\_embedding\_v1, body\_embedding\_v2). Vector indexes exist on both.

**Phase 2 — New writes use new model:** any INSERT or UPDATE generates v2 embeddings. Existing rows still have v1 embeddings; new rows have v2 embeddings.

**Phase 3 — Background backfill:** a migration job scans existing rows with v1 embeddings, generates v2 embeddings. Progress tracked in \_system.embedding\_migrations.

**Phase 4 — Dual-read during migration (default):** query embedded with both models, both vector indexes searched, results fused. Most user-friendly: correct results throughout migration. Costs 2x embedding calls per query, 2x index searches. Mitigated by query-embedding cache.

**Phase 5 — Completion:** when backfill is done, ALTER ... COMPLETE MIGRATION drops the v1 column and index, removes v1 from active models, future queries use only v2.

**Rollback:** ALTER ... ABORT MIGRATION drops v2 column and index, returns to pre-migration state.

## **Source updates during migration**

If the user updates the source text while migration is in progress, both versions need regeneration. Two embedding jobs enqueued — one for v1 (yes, the model we're migrating away from; we need to keep v1 consistent until migration completes), one for v2. Doubles embedding cost for source updates during migration. Acceptable for short-term events.

## **Local-only models for sensitive data**

Some columns should never have their embeddings sent to external APIs (healthcare, financial, regulated). We support local\_model\_only constraint per column. The system enforces that the column can only use models marked as local \= true in the model registry. Migration to a non-local model is rejected.

| Locked Decisions — Embedding Lifecycle |
| :---- |
| EMBED FROM as declarative schema feature; system owns generation, regeneration, migration. Deferred generation as default; inline mode as opt-in per table. Embedding job queue as system table with SKIP LOCKED for worker concurrency. Workers batch by model\_id; respect rate limits via per-model token bucket. Embedding cache per-model keyed by BLAKE3/xxh3 hash of source text; query-time and write-time hits. Error categorization: transient (retry with backoff), persistent (mark failed), hard (mark failed). Optional per-table embedding\_failure\_webhook. Dual-read migration model: ADD MIGRATION → backfill → COMPLETE MIGRATION (or ABORT). During migration: both models active for new writes; query embedded with both, fused results (default). Migration progress tracked in system table. Source updates during migration regenerate both embedding versions. Model registry with model\_id (system) and name (user-assigned, versioned). Per-model concurrency, batch size, rate limit configuration. Local-model-only constraint per column for sensitive data. |

# **Part XIV — The Wire Protocol and SDK**

The wire protocol is the public surface. Everything we have built becomes accessible only through this layer. Get it wrong and the architectural sophistication does not matter; get it right and we tap into existing ecosystems for free.

## **Two surfaces: native gRPC \+ partial Postgres wire**

LL ships both, partially: native protocol with first-class clients in Python and TypeScript (the primary interface, optimized for AI workloads), and Postgres wire compatibility for SELECT queries and basic writes (covers BI tool integration and most application use cases). What we do NOT do in v1: Postgres COPY, replication protocol, full procedural language compatibility. We are honest about being a partial Postgres-wire compatibility, not a drop-in replacement.

The trade: the Postgres surface gives us the BI ecosystem and ORM compatibility. The native surface gives us AI-engineer-friendly ergonomics and efficient binary results. Internally the pipeline is protocol-agnostic; the wire layers translate to logical plan / SQL.

## **Native protocol: gRPC \+ Protobuf \+ Arrow IPC**

gRPC with bidirectional streaming. Battle-tested, supports streaming results (essential for large query results), HTTP/2 transport (multiplex many concurrent queries on one connection), code generation for clients in every major language. Protobuf for wire format (binary, compact, schema-evolved cleanly). Arrow IPC for result data (zero-copy deserialization into Polars/Pandas).

Service definition sketch:

service LL {  
    rpc Query(QueryRequest) returns (stream QueryResult);  
    rpc ExecutePlan(PlanRequest) returns (stream QueryResult);  
    rpc Prepare(PrepareRequest) returns (PrepareResponse);  
    rpc Execute(ExecuteRequest) returns (stream QueryResult);  
    rpc Begin(BeginRequest) returns (BeginResponse);  
    rpc Commit(CommitRequest) returns (CommitResponse);  
    rpc Rollback(RollbackRequest) returns (RollbackResponse);  
    rpc BulkInsert(stream BulkInsertRequest) returns (BulkInsertResponse);  
    rpc DescribeTable(DescribeRequest) returns (DescribeResponse);  
    rpc ListModels(ListModelsRequest) returns (ListModelsResponse);  
    rpc EmbedText(EmbedRequest) returns (EmbedResponse);  
}

Connection lifecycle: client opens HTTP/2 connection; authenticates via gRPC interceptor; issues queries; server streams results back; multiple concurrent queries multiplex on one connection; connection persists until close or idle timeout.

## **Postgres wire compatibility**

What we implement: startup handshake (parameter negotiation, SSL/TLS upgrade, authentication), simple query protocol (SELECT/INSERT/UPDATE/DELETE), extended query protocol (Parse/Bind/Execute used by modern drivers), parameter binding via binary format, result transmission, transaction control (BEGIN, COMMIT, ROLLBACK), error responses with Postgres SQLSTATE codes.

What we don't: COPY protocol (defer), replication protocol (not v1), WITH HOLD cursors (uncommon), LISTEN/NOTIFY (defer), PL/pgSQL execution, byte-identical Postgres system catalogs.

Implementation: a separate listener on Postgres port (5432 by default). Postgres clients connect, we run the wire state machine, parse SQL, dispatch through the same internal pipeline as native clients. Internal pipeline does not know which protocol the query came from. All protocol differences are at the wire layer.

## **Authentication**

v1 ships: username/password (Argon2id-hashed, stored in catalog) and API keys (long random strings, hashed, with optional scopes and expiration). SCRAM-SHA-256 supported on the Postgres wire endpoint. OAuth/OIDC and mTLS deferred to v2.

## **Connection pooling and management**

Server-side: max\_connections (default 1000), each connection has session state. HTTP/2 multiplexing on native protocol makes connection counts modest. Client-side: SDKs include built-in connection pooling (default pool size 10). Idle timeout default 5 minutes.

## **Prepared statements**

Both protocols support them. Parse and plan once, execute many times. Critical for ORM performance. The prepared statement cache lives in the session. Plans are stored as physical plans (ready to execute). Re-planning on schema change.

## **Streaming bulk insert (native only)**

BulkInsert(stream BulkInsertRequest) accepts streamed Arrow RecordBatches. Server appends them to the target table in batched commits. Configurable commit batch size (default 1000 rows or 1MB). Throughput limited by WAL fsync and memtable flush rates — \~100K rows/sec for small rows, less for vector-heavy.

## **SDKs**

**Rust SDK (foundation):** pure Rust, async, fast. Used directly by Rust applications and as the basis for other language bindings.

**Python SDK (PyO3 wrapper around the Rust client):** v1 ships SQL-based API (string queries, Arrow result deserialization). v1.5+ adds the fluent DSL that builds plans directly.

**TypeScript SDK (napi-rs wrapper):** mirror of Python. v1 SQL-based, DSL later.

The wrap-the-Rust-client approach: one set of protocol logic to maintain, optimal performance everywhere, less code to write. Alternative (hand-write each SDK natively) gives more per-language idiomatic ergonomics; we trade that for consistency.

## **Error model**

Each error carries a stable machine-readable code, a human-readable message, optional details (query position for parse errors, conflicting row for serialization errors), and a category (transient/permanent for retry logic). On Postgres wire: SQLSTATE codes. On native: LL codes (with SQLSTATE mapping for shared categories). Clients need to know which errors are transient (retry with backoff) vs permanent (don't retry).

## **Query response metadata**

Every query response includes: query ID (UUID, for tracing), execution time breakdown (planning, execution, network), rows examined/returned/filtered, memory peak, spill bytes, buffer pool and embedding cache hit rates, files accessed, operators in plan. This is the implicit observability interface (EXPLAIN is the explicit one).

## **Versioning**

Native gRPC: each service method has a version in the request; server supports last N versions. Postgres wire: PG 15+ protocol 3.0 (stable). SDKs: semver, major bumps for breaking changes (we try hard to avoid). Compatibility window: support last 3 major versions.

| Locked Decisions — Wire Protocol and SDK |
| :---- |
| Native protocol: gRPC with bidirectional streaming, Protobuf wire format, Arrow IPC for result data. Postgres wire compatibility: simple \+ extended query protocols, transaction control, prepared statements, error mapping. Defer COPY, replication, full PL/pgSQL. Internal pipeline protocol-agnostic; wire layers translate to logical plan / SQL. Authentication: username/password (Argon2id) \+ API keys in v1; OAuth/mTLS in v2. Connection management: HTTP/2 multiplexing, configurable pools, idle timeouts. Streaming results via Arrow RecordBatches embedded in protocol messages. Prepared statements supported in both protocols. Streaming bulk insert in native protocol; INSERT-based for Postgres wire. SDKs: Rust client as foundation; wrap for Python (PyO3) and TypeScript (napi-rs). v1: SQL-based client APIs; DSL fluent API in v1.5+. Error model with stable codes, retry hints, SQLSTATE mapping. Query response metadata for implicit observability. Semver for SDKs; support last 3 major client versions. |

# **Part XV — Observability**

A database without good observability is unsupportable in production. The operator who cannot diagnose a slow query cannot fix it; the team that cannot see embedding pipeline lag cannot capacity-plan; the SRE who cannot trace a request cannot debug an outage. Observability is not a feature that gets bolted on; it is a layer that is designed alongside the rest of the system.

## **EXPLAIN — the developer's primary tool**

Every database has EXPLAIN. Most are terrible. Postgres's EXPLAIN ANALYZE is the gold standard despite verbosity; pgvector's EXPLAIN is essentially useless because it shows 'Bitmap Index Scan on vector\_idx' with no useful information about HNSW behavior. For LL, EXPLAIN matters more than usual because the planner makes multi-modal decisions users will second-guess. Why did it pre-filter instead of using HNSW? That question needs an answer the user can verify.

### **EXPLAIN modes**

* EXPLAIN \<query\> — shows the physical plan with cost estimates; no execution.

* EXPLAIN ANALYZE \<query\> — executes the query, shows the plan with actual statistics alongside estimates.

* EXPLAIN VERBOSE \<query\> — includes column projections, expressions, cost breakdowns, statistics references.

* EXPLAIN (FORMAT JSON) \<query\> — machine-readable output for tooling.

### **What the output shows**

The output presents each operator with: operator name, cost estimate vs actual time, strategy choice (for operators with multiple physical implementations), selectivity estimate vs actual, file pruning statistics (how many files skipped, how many touched), buffer pool cache behavior, distance computation count for vector operators, and memory/timing. The most important diagnostic signal is showing estimated vs actual side by side for cardinality and selectivity. If the estimate was 1,000 and the actual was 100,000, the planner picked the wrong plan; that's the place to focus.

For operators with variants such as PredicatedVectorSearch, the strategy choice is surfaced: 'I chose IntegratedFilteredHNSWSearch because selectivity was estimated at 3.4%.' This is the kind of transparency that makes the system tunable rather than mystical.

### **HNSW-specific EXPLAIN extensions**

Vector searches show per-file detail: layers traversed, upper-layer distance computations, layer-0 distance computations, candidates returned, and the recall-at-10 estimate stored in the file's footer at build time. The recall estimate is surfaced so users know whether to trust the result; a file with a 78% recall estimate gives a different signal than one at 98%.

### **Visualization**

Text EXPLAIN is good for terminals. For complex plans users want visualization. Query plans are exposed as structured JSON via EXPLAIN (FORMAT JSON); a CLI tool renders this as ASCII tree (default), Mermaid diagram, or graphviz DOT. A future web UI lets users paste a JSON plan and get an interactive visualization. The UI is post-v1 tooling; the JSON output is v1.

## **Per-query telemetry**

Every executed query produces a telemetry record. The record contains: query ID (UUID for tracing), user and session identity, started/completed timestamps, query text (truncated to a configurable length), query hash (for grouping similar queries), protocol, isolation level, planning time breakdown, execution time, rows examined/returned/filtered-by-MVCC, memory peak and spill bytes, buffer pool hits/misses, embedding cache hits/misses, files scanned/pruned, vector distance computations performed, text posting decode bytes, graph edges traversed, embedding API calls and their latency, status, and error details if any.

This record goes to multiple destinations: an in-memory ring buffer of the last N queries (default 10K, queryable via \_system.recent\_queries); a query log file written as JSON lines or Parquet, rotated daily; metrics aggregation (counters and histograms updated per query); and optionally an external trace destination (OpenTelemetry, Jaeger, Datadog) when configured. The cost of collecting this telemetry is a few microseconds per query — negligible.

## **The slow query log**

A separate, distilled stream: queries exceeding a configurable threshold (default 1 second, configurable per user/table). Slow queries get extra detail captured — full query text (not truncated), full EXPLAIN ANALYZE output, parameter values (sanitized for sensitive types), and system state at execution (CPU load, memory pressure, concurrent queries). The slow query log is the primary debugging tool when 'something is slow.' Format is JSON lines, rotated by size and time, optionally shipped to external systems via OTel.

## **Query history aggregation**

Aggregated query statistics are persisted in \_system.query\_stats — query hash, sample query text, execution count, total/mean/p50/p95/p99/max execution time, rows returned total, error count, first and last seen timestamps. This is updated by a background aggregator reading the query log. It is the LL equivalent of pg\_stat\_statements, and it is beloved by every DBA who has used the Postgres version. Queries like 'show me the top 20 queries by total time spent' are SQL one-liners against this table.

## **Prometheus metrics**

System-wide aggregate metrics exposed on a configurable HTTP endpoint (default port 9090\) in Prometheus exposition format. The metrics cover: query rate broken down by status; query latency histograms; memory usage by component (buffer pool, query memory, catalog); buffer pool hit rate; disk I/O rate and bytes; compaction queue depth, ongoing compactions, throughput; WAL bytes per second and fsync latency; memtable count and total size; file counts per tier; connection count and active queries; embedding job queue depth and processing rate per model; embedding API latency per model; cache hit rates (buffer pool, embedding cache, plan cache).

Names follow Prometheus conventions: ll\_query\_duration\_seconds, ll\_buffer\_pool\_hit\_rate, etc. Each metric has clear name, labels where relevant, and help text. Label cardinality is bounded explicitly to avoid Prometheus's classic cardinality explosion.

## **Distributed tracing via OpenTelemetry**

For requests that span multiple operations — transactions with multiple queries, queries calling external embedding APIs — tracing connects the dots. OpenTelemetry is the standard. Instrumentation covers: query lifecycle (parse, plan, execute) as a span; each major operator execution as a child span; external API calls (embedding services) as child spans; I/O operations (page reads, compactions) as child spans (sampled, not all). Output goes to whatever backend the user configures (Jaeger, Tempo, Datadog, Honeycomb).

Sampling: 100% is too expensive for production. Default is 1% sampling of normal queries and 100% sampling of slow queries (over the threshold) and errors. Configurable. Trace context propagates from the gRPC interceptor through the system to outgoing external API calls; users can trace from their application through LL to embedding providers.

## **System tables — everything is queryable**

Anything observable is queryable as a system table. The pattern is consistent and complete:

* \_system.tables — table list, sizes, file counts.

* \_system.columns — column definitions.

* \_system.indexes — index status, types.

* \_system.files — file inventory per table, tier, size, LSN range.

* \_system.compactions — compaction history and ongoing.

* \_system.memtables — active memtable status.

* \_system.transactions — active transactions, age, snapshot LSN.

* \_system.recent\_queries — ring buffer of recent queries.

* \_system.query\_stats — aggregated query statistics.

* \_system.embedding\_jobs — pending, processing, failed embedding jobs.

* \_system.embedding\_migrations — migration status.

* \_system.models — registered models, status, error rates.

* \_system.connections — active connections.

* \_system.buffer\_pool\_stats — per-shard cache stats.

* \_system.locks — held row locks (for conflict diagnosis).

* \_system.config — current configuration values.

* \_system.health — component health checks.

This is the pg\_catalog pattern; it is why Postgres is debuggable — every internal state is reachable via SQL. LL inherits this discipline. Observability uses the same query engine as user data; no separate observability backend, no separate query interface.

## **Embedding pipeline observability**

Embedding generation is asynchronous, can fall behind, can fail; users need visibility. \_system.embedding\_jobs covers per-job visibility. \_system.embedding\_pipeline\_stats exposes the aggregate per-model picture: jobs pending, in-progress, failed in last hour; queue depth and wait times; throughput; average and optimal batch size; API latency p50/p99; API error rate; cache hit rate; estimated time to drain the queue.

The 'estimated time to drain the queue' is genuinely useful — operators see 'the queue has 50,000 jobs and is processing 100 per second; ETA 8 minutes' and know whether to act. Per-model granularity is critical because different models have different characteristics; one slow model's pipeline may be unhealthy while another fast model's is fine.

## **Health checks**

Standard HTTP endpoints for orchestrators (Kubernetes, load balancers):

* GET /health/liveness — is the process alive? Always 200 if the server can respond.

* GET /health/readiness — is the process ready to serve traffic? 200 if catalog is loaded, buffer pool is warmed, accepting connections. 503 during startup or degraded states.

* GET /health/components — detailed per-component health. Returns JSON with status of each subsystem.

## **Configuration introspection**

\_system.config exposes every setting, its current value, whether it is default or overridden, and where the override came from (config file, environment, runtime SET command). This sounds trivial but is critical for 'why is this behaving differently in production than in staging?' debugging. The first question is always 'what is different?' and configuration is usually the answer.

## **Audit log**

A separate stream captures security and compliance events: authentication (login, logout, failed attempts), authorization (privilege grants and revocations), schema changes, model lifecycle events, sensitive table accesses (configurable), and configuration changes. Stored in \_system.audit\_log, append-only, retained per compliance requirements. Optionally shipped to external SIEMs via OpenTelemetry or syslog.

Audit log retention is configurable (default 1 year). Hash-chained audit log entries — where each entry includes a hash of the previous, making silent deletion detectable — are deferred to v2.

## **Honest tradeoffs**

* **System table queries through main engine.** Introspection queries compete with user queries for resources. Mitigation: dedicated thread pool for system queries, or recommend heavy introspection during off-hours.

* **Telemetry storage growth.** Query log, audit log, query stats accumulate. Rotation and compression help; retention policies are required. Defaults: 30 days query log, 1 year audit log, query stats aggregated indefinitely.

* **Trace sampling default of 1%.** Conservative. Workloads that want richer data raise it. The default reflects expected production behavior.

| Locked Decisions — Observability |
| :---- |
| EXPLAIN with PLAN, ANALYZE, VERBOSE, JSON modes; shows estimated vs actual, strategy choices, per-operator stats. Multi-modal-specific EXPLAIN: HNSW layer traversals, file pruning, recall estimates, selectivity stats. Per-query telemetry record with comprehensive metadata; in-memory ring buffer \+ query log file \+ metrics \+ optional OTel. Slow query log with full EXPLAIN ANALYZE captured for queries exceeding threshold. Aggregated query history via \_system.query\_stats (pg\_stat\_statements equivalent). Prometheus metrics on HTTP endpoint with standard naming conventions and bounded label cardinality. OpenTelemetry tracing with configurable sampling (1% normal, 100% slow/error). All internal state introspectable as \_system.\* SQL tables. Per-model embedding pipeline statistics with queue depth, drain ETA. Standard liveness/readiness/component health endpoints. Configuration introspection via \_system.config with provenance. Audit log for security and compliance events. Hash-chained audit log deferred to v2. |

# **Part XVI — Recovery and Durability**

A database's durability guarantee is a contract: 'if I returned commit success and the data is not here, that is a bug, not user error.' Setting this precisely matters more than almost any other piece of the system, because the value of the system is bounded above by how seriously users trust the contract.

## **The durability contract**

* **Acknowledged writes survive process crashes.** If commit success was returned and the process is killed (kill \-9, OOM kill, segfault), the write is recoverable on restart.

* **Acknowledged writes survive power loss.** If commit success was returned and the machine loses power, the write is recoverable on restart — assuming the underlying storage hardware honors fsync.

* **Acknowledged writes survive single-disk corruption.** If a sector goes bad after a successful write, checksums detect it; recovery surfaces the failure clearly rather than silently corrupting.

* **Acknowledged writes do not survive:** catastrophic hardware failure (disk destruction), multiple simultaneous failures, lying storage hardware that ignores fsync, filesystem corruption beneath us. These require backup/restore or replication (v2).

This is the standard single-node durability story. Postgres makes the same promises. LL honors the same constraints.

## **The fsync discipline**

fsync() is the system call that asks the OS to flush data to durable storage. Without it, writes might live only in OS cache — fast, but lost on crash. Where LL fsyncs:

* **WAL writes at commit.** This is the durability moment. A commit returns success only after the WAL fsync completes. Everything else can be lazy.

* **File close after flush.** When a memtable flushes to a new file, the file is fsynced before the catalog is updated to reference it. Otherwise a crash could leave a catalog pointer to a half-written file.

* **Catalog updates.** redb handles its own fsync internally; LL trusts redb's transaction model.

* **Compaction output.** Same as memtable flush: fsync new files before swapping the catalog.

Reads never trigger fsync. Memtable updates do not fsync (durability comes from the WAL). Buffer pool dirty pages do not exist; pages are read-only because files are immutable. The WAL fsync is the only hot-path fsync.

## **Group commit**

A naive WAL fsync per commit limits throughput to 1 over fsync latency. On modern NVMe, fsync is around 100μs; that is 10K commits per second maximum — not enough for high-throughput workloads. Group commit fixes this:

Commits write their WAL records to an in-memory buffer and join the current commit group. A dedicated WAL writer thread picks up the group, writes all records to disk, issues one fsync, and on completion notifies all commits in the group. Each commit sees its commit return after the group's fsync completes. With 100 concurrent commits batched together, throughput becomes 100 over fsync latency — around 1M commits per second on NVMe.

Latency per commit is slightly higher than one fsync because of group formation wait (typically under 1 ms). The implementation uses a lock-free MPSC queue where commits push their WAL records and the writer thread is the single consumer. The writer adaptively waits up to N microseconds (configurable, default 100\) for more commits, then commits the group when N expires or when group fills (configurable max group size).

## **The WAL file structure**

WAL is a sequence of records appended to one or more files. Each file starts with a header (magic, version, base LSN), followed by variable-length records (LSN, txn\_id, type, payload, CRC32), and ends with a footer written at fsync (last record LSN). Records are length-prefixed; CRC32 per record detects corruption. Files rotate at a size threshold (default 64MB); new writes go to a new file; old files are retained until checkpointed.

File naming is monotonically increasing IDs — wal\_000001.log, wal\_000002.log, etc. Easy to enumerate, easy to reason about, easy to clean up.

## **WAL record types**

Logical logging means WAL records describe what changed, not how at the page level. Record types: INSERT\_ROW, UPDATE\_ROW, DELETE\_ROW, COMMIT, ABORT, SCHEMA\_CHANGE, CHECKPOINT, COMPACTION\_COMMIT, FILE\_REGISTER, FILE\_DELETE. The high-frequency types are INSERT\_ROW, UPDATE\_ROW, DELETE\_ROW, COMMIT. Each carries the LSN assigned at write time. LSNs are monotonically increasing across all records — one global sequence per database.

## **Checkpoints**

Without checkpoints, recovery has to replay the entire WAL from the beginning of time — unacceptable as the WAL grows. A checkpoint marks a point at which all data before it is fully durable in files (not just in WAL). After a checkpoint, WAL records before the checkpoint LSN can be discarded.

Checkpoint procedure: mark checkpoint start LSN; force flush all memtables to files (write rows, build indexes, fsync); update catalog to reference new files; mark checkpoint complete LSN; write CHECKPOINT record to WAL with the checkpoint LSN; fsync the WAL; WAL files containing only records before the checkpoint can now be deleted.

Recovery only needs to replay from the most recent CHECKPOINT record forward. Default trigger: every 5 minutes or when WAL exceeds 1GB, whichever comes first. Configurable. Checkpoint is a background operation; it does not block writes — new writes during checkpoint go to a new memtable.

## **Crash recovery**

What happens when the process restarts after a crash:

Phase 1: Open the catalog. Open the redb catalog file. Read table list, file list, model registry into memory. Check for consistency (catalog file integrity).

Phase 2: Find the recovery starting point. Scan the WAL directory for files. Read the most recent CHECKPOINT record. Recovery starts at the CHECKPOINT's start LSN.

Phase 3: Identify active transactions at crash time. Scan from checkpoint forward. Build a set of all transactions seen. For each, track whether a COMMIT or ABORT record was seen. Transactions with COMMIT are committed; their writes are durable. Transactions with ABORT or no completion record are aborted; their writes are dead.

Phase 4: Replay the WAL. For each record from checkpoint forward, look up the transaction's status. Skip writes from aborted transactions. Replay writes from committed transactions into the memtable, resolving xmin to commit\_lsn. Apply schema changes, file registrations, compaction commits to the catalog.

Phase 5: Verify file integrity. For each file referenced in catalog, open it and verify the footer checksum. Files that fail verification are marked suspect; the error is surfaced. Suspect files can be recovered from backup or operated on manually.

Phase 6: Resume operations. Open network listeners. Begin accepting connections. Background processes (compaction, embedding workers) start.

Total recovery time is dominated by Phase 4 (WAL replay). For 1GB of WAL containing 1M records, replay is around 5-10 seconds on modern hardware. For 10GB of WAL, around 1 minute. Bounded by checkpoint frequency.

## **Torn writes**

Disk hardware writes in sectors (typically 512B or 4KB). If a write is in progress and power is lost, the sector may be partially updated — torn. Half the new data, half the old data, corruption.

For WAL records crossing sector boundaries: a 1KB WAL record may span two sectors. If only one is written before crash, recovery has half a record. LL's defense is CRC32 per record. On replay, if CRC does not match, the record is corrupt; replay stops at the first corrupt record (which must be the last record, since all previous ones were checksummed successfully). All transactions after the corrupt record are aborted. This is correct: never partially apply a corrupted record.

For data file pages: files are designed immutable — once written, never modified. The only torn-write risk is during the initial write of a file. Procedure: write the file to a temporary name (file\_abc.tmp); fsync the file; rename to the final name (atomic on POSIX filesystems); update catalog to reference the new file; fsync the directory. If a crash happens between temporary and rename, the temporary file is incomplete and is deleted on startup. If between rename and catalog update, the file exists but is not catalog-referenced; it is deleted on startup. After catalog update, the file is committed and cannot be torn because it is never modified again.

## **Configurable durability levels**

Some users have weaker durability requirements. LL offers four levels:

* **durability \= full** (default): WAL fsync per group commit. Acknowledged writes survive crashes.

* **durability \= sync**: WAL written but fsync deferred to a background flusher every N milliseconds. Acknowledged writes survive process crashes but might not survive OS crashes.

* **durability \= async**: WAL written to memory buffer, flushed periodically. Acknowledged writes are not durable until flush completes.

* **durability \= none** (no WAL): writes go straight to memtable, no WAL. On crash, all uncommitted data is lost. Useful for ephemeral testing only.

This is per-session or per-table. Most users use the default. Users who explicitly opt out know what they are trading. This is the same model as MongoDB write concern, Cassandra consistency levels — convention, not invention.

## **Backup**

Two backup strategies. Logical backup dumps all data as SQL or a wire-format file — slow, large, but portable across versions and machines. Physical backup copies the data files and catalog directly — fast, exact, but version-specific. v1 ships physical backup with two modes:

* **Cold backup.** Shut down the database, copy all files (data directory, WAL, catalog), restart. Simple, safe, requires downtime.

* **Hot backup.** While running, snapshot consistently: trigger a checkpoint, note the checkpoint LSN, copy all files referenced in the catalog at that LSN, copy WAL files from the checkpoint onward. The copy itself is filesystem-level (cp, rsync, ZFS snapshot, btrfs snapshot). LL does not implement its own copy protocol.

Hot backup works because files are immutable — once in the catalog, they never change; copying is safe. The catalog itself is a small file (redb) and copying it captures a consistent snapshot. For incremental backup, only files added since last backup need copying — incremental is straightforward because files are immutable.

## **Restore and point-in-time recovery**

Restore: stop the database, restore the data directory from backup, start the database; WAL replay completes recovery from the backup's LSN. For point-in-time recovery (PITR), if WAL files are retained beyond the checkpoint-driven defaults, recovery can replay WAL up to any target LSN. PITR retention is configurable (default 7 days, configurable). A verification tool ships with the system: given a backup directory, it runs a test restore in a sandbox and validates queries against expected results. Backups that do not restore are not backups.

## **The lying-storage problem**

Some hardware lies about fsync. Cheap consumer drives sometimes return success on fsync before the data actually hits the platters; if power fails in the window, data is lost despite our fsync. This is fundamentally outside LL's protection. The promise is 'if fsync truly succeeds, the data is durable.' If hardware lies, no defense exists.

Mitigation: document the assumption that storage honors fsync; recommend enterprise SSDs with power-loss protection (capacitors that flush volatile cache on power loss); ship a diagnostic tool that tests whether the user's storage honors fsync. This is the same situation Postgres and every other database is in; there is no magic.

## **Honest tradeoffs**

* **Group commit latency cost.** Individual commits wait up to 100μs for group formation. For very-low-latency workloads this is annoying. Tunable.

* **WAL retention for PITR.** Keeping 7 days of WAL on disk costs storage. Users who do not need PITR configure shorter retention.

* **Recovery time bounded by WAL size.** A long-running database with high write rate generates lots of WAL. Recovery can take minutes for big systems. Mitigated by checkpoints but not eliminated.

* **Logical backup deferred to v2.** Physical backups are not portable across major versions. Users upgrading need to keep the old binary around for restores.

| Locked Decisions — Recovery and Durability |
| :---- |
| Durability contract: acknowledged writes survive process crashes and power loss assuming honest hardware fsync; checksums detect corruption. One WAL with logical record types: INSERT/UPDATE/DELETE\_ROW, COMMIT, ABORT, CHECKPOINT, schema and file metadata events. LSN is the global monotonic ordering primitive; CRC32 per WAL record. WAL fsync at group commit boundary; configurable wait up to 100μs default. Files written via temp-then-rename; immutable after rename; three-tier checksums (file, section, page). Checkpoint procedure: flush memtables, fsync files, write CHECKPOINT record; bounds recovery time. Default checkpoint trigger: every 5 minutes or 1GB of WAL. Recovery: open catalog → identify transactions → replay WAL from checkpoint → verify file integrity → resume. Configurable durability levels: full (default) / sync / async / none. Hot backup procedure: checkpoint, copy data files \+ WAL since checkpoint, no downtime. WAL retention configurable for PITR (default 7 days). Backup verification tool ships with system. Documented hardware assumptions: requires honest fsync; recommend enterprise SSDs with power-loss protection. |

# **Part XVII — Operations**

This is the layer where everything LL designs meets the people who have to run it in production. The architecture might be beautiful, but if the deployment story is bad, no one ships it. The right test for this section is: can a thoughtful SRE go from 'I want to evaluate LL' to 'we are running it for our application' in a week without surprises?

## **Release artifacts**

* **A single static binary.** The Rust compile target. Statically linked where possible (musl on Linux for true static, or dynamically linked against glibc with versioned deps as fallback). Sizes around 80-120MB. Contains the database engine, the SQL parser, the wire protocol servers, the embedding pipeline. The principle: one binary, one process. No microservices, no sidecar containers, no internal RPC. Operations is dramatically simpler when there is one thing to deploy.

* **Platform builds.** Linux x86\_64 (the workhorse), Linux aarch64 (Graviton, Ampere — common in cloud), macOS Apple Silicon (developer machines). Windows is not v1; server-side databases on Windows are uncommon enough to punt.

* **Container images.** Official Docker images on Docker Hub and GitHub Container Registry. Minimal base (distroless or Alpine-with-musl). Multi-arch manifests.

* **Kubernetes manifests.** Helm chart with reasonable defaults.

* **CLI tool.** Same binary, different entry points: ll server, ll client, ll admin. The CLI handles backups, restores, schema dumps, configuration validation, diagnostics.

* **Language SDKs.** Python (PyPI), TypeScript (npm), Rust (crates.io) per the wire protocol section. Build matrices for the wrapper SDKs target every supported platform.

* **Helm chart and Terraform modules.** For Kubernetes and AWS/GCP/Azure deployments.

* **Documentation site.** Static site, versioned, searchable. Postgres-quality is the standard; nobody adopts a database with bad docs.

## **Deployment shapes**

**Single-process embedded** (like SQLite, DuckDB). The application links the LL library directly; no separate server. For: edge applications, mobile, single-tenant tools, agentic apps with embedded knowledge stores. Supported in principle but not the primary v1 deployment.

**Single-node server** (like Postgres on one box). The LL binary runs as a daemon; applications connect via wire protocol. For: most production deployments below the scale ceiling, dev and staging environments, smaller production workloads. This is the v1 sweet spot. Capacity ceiling: comfortably supports up to 5-10TB of data per node; up to 50TB with appropriate hardware. Beyond that, distribution becomes necessary.

**Single-node with read replicas** (v2 architecture). Primary node accepts writes; replicas accept reads. Sync via WAL streaming. For: scaling read throughput, geographic read locality, high availability. Many production databases run for years in this configuration without ever needing full distribution.

## **Hardware sizing**

* **Storage.** NVMe SSD is mandatory. Spinning disks make compactions painful and HNSW page reads catastrophic. Cloud equivalent: gp3/io2 on AWS, pd-ssd/pd-extreme on GCP. For high write throughput, local NVMe (instance store) gives best fsync latency.

* **RAM.** Buffer pool target 30-50% of total data size, capped at available RAM, plus 1-4GB per concurrent heavy query plus 10% headroom for OS. For 100GB dataset with 20 concurrent queries: \~50GB buffer pool \+ 40GB query memory \+ 10GB headroom \= 100GB RAM minimum.

* **CPU.** Scale with query concurrency and embedding workload. Vector distance computation is the hottest path. AVX-512 gives 2x speedup over AVX2. GPU resources for embedding workers when local GPU models are used.

* **Network.** Modest bandwidth for clients (1-10 Gbps typical). For replicas, high bandwidth for WAL streaming.

Sizing guidance is published per common workload (RAG, semantic search, hybrid queries). Users plug in their data size and QPS, get a recommended instance type.

## **Configuration management**

Configuration sources, in order of precedence (highest first):

* Runtime SET commands (highest).

* Per-user settings in catalog.

* Per-table settings in catalog.

* Configuration file (ll.conf in TOML format).

* Environment variables (mostly for secrets).

* Compiled-in defaults (lowest).

Every setting has a sensible default; configuration is for tuning, not for making the system work. The CLI ships ll admin config validate (checks for typos, conflicting settings, obvious mistakes) and ll admin config show (prints current effective config with provenance — where each value came from).

Most settings can be changed via SET without restart. Settings requiring restart (listen addresses, data directories, compaction worker count, buffer pool size) are clearly marked in documentation and config introspection. Per-session SET is immediate; per-system SET propagates within seconds.

## **Upgrades**

* **Patch versions (x.y.Z bumps):** bug fixes only, fully backward compatible. File format, wire protocol, configuration: unchanged. Procedure: stop, replace binary, start.

* **Minor versions (x.Y.z bumps):** new features, backward compatible. File format extended additively, wire protocol extended additively, configuration additive. Procedure: same as patch.

* **Major versions (X.y.z bumps):** breaking changes possible. File format may have changed, migration provided. Wire protocol may have changed, at least one prior version maintained. Configuration may require changes, migration tool helps. Procedure: backup, run migration tool, upgrade, restart.

Major versions are rare — once every 1-2 years. Most upgrades are minor or patch and should be trivial. File format migration ships as an admin command (ll admin migrate) that converts old files in-place or in-parallel. Compaction can do this automatically as files get rewritten. Zero-downtime upgrades require v2 replicas; single-node has downtime during binary swap.

## **Operational CLI**

* ll server start | stop \--graceful | status | reload — lifecycle and SIGHUP-equivalent reload.

* ll admin backup \--to /backups/today — physical backup to local or S3.

* ll admin restore \--from /backups/today — restore.

* ll admin verify-backup \--path /backups/today — verify a backup.

* ll admin status | diagnose — system state summary and comprehensive health check.

* ll admin slow-queries \--since 1h — recent slow queries.

* ll admin compactions | embedding-pipeline — subsystem status.

* ll admin vacuum | reindex | migrate | checkpoint — maintenance.

* ll admin config validate | show \[--provenance\] — configuration tooling.

* ll admin schema dump | apply | diff — schema management.

* ll admin user create | grant | list — user and permission management.

CLI is stable, well-documented, gracefully handles errors, outputs JSON when piped (for scripting).

## **Monitoring patterns**

The standard golden signals: latency (p50/p95/p99), traffic (queries per second), errors (rate, types), saturation (CPU, memory, buffer pool hit rate, disk space, connection count). LL-specific signals: compaction queue depth and rate, embedding pipeline queue depth per model, memtable count and pressure, WAL fsync latency, file count per tier (leading indicator of compaction lag).

Grafana dashboards (JSON definitions) ship with the release. Plug into a Prometheus instance scraping the metrics endpoint; dashboards work out of the box. AlertManager rules ship with sensible defaults: critical alerts for WAL fsync p99 \> 100ms (storage problem), buffer pool hit rate \< 50% (undersized), connection count \> 90% capacity. Warning alerts for compaction lag \> 1 hour, embedding queue \> 10K, disk usage \> 80%.

## **Logs**

Three log streams: server log (operational events — startup, shutdown, configuration loaded, compactions, errors); slow query log (per the observability section); audit log (security and compliance events). All logs go to stdout/stderr by default (12-factor app principle), with optional file destinations. Standard log levels (debug, info, warn, error, fatal). Configurable per-component log level.

Log shipping is not implemented by LL. Users use whatever they prefer (Vector, Fluent Bit, Filebeat, Logstash). Log format is structured JSON that works with any of them.

## **Kubernetes deployment**

The deployment unit: a StatefulSet with one or more replicas. v1 is single-instance. v2 adds read replicas (additional pods). Persistent volumes for data directory and WAL directory (often separate for performance — WAL on fast local SSD, data on network-attached SSD).

The Helm chart parameterizes everything reasonable: storage class for each volume, resource requests/limits, configuration via values.yaml, secrets via Kubernetes Secrets, service definitions for native and Postgres protocols, optional monitoring sidecar (Prometheus exporter is in-process but pod-level annotations for scraping). Documentation covers common patterns: using GP3 in EKS, using node-local NVMe for WAL, hot-standby coordination.

## **Cloud deployment**

Terraform modules ship for AWS (EC2 with EBS), GCP (GCE with persistent disks), Azure. Modules handle instance sizing, disk provisioning, networking, security groups, IAM roles, backup to object storage. v1 explicitly does not ship a managed service; Terraform modules let users run on their cloud with minimal config. A fully managed service is a future business decision.

## **Failure modes that are not crashes**

* **Disk filling up.** At 90% capacity: switch writes to throttle mode (each commit waits an increasing duration); log warnings prominently; trigger emergency compaction. At 95%: stop accepting writes, reads continue. At 99%: read-only mode. Gives operators time to add storage or clean up.

* **Memory pressure.** When buffer pool cannot satisfy demand and OS starts swapping, surface a warning. If queries start failing OOM, reject new queries, allow current to finish. Recommend remedies in error messages. No auto-resize; explicit operator action.

* **Embedding model unavailable.** Workers retry with backoff. After max retries, jobs are marked failed. New inserts: embedding column null; row succeeds. Operator notification via metrics and logs. When API returns: workers automatically resume. The database stays up even when external dependencies fail.

* **Slow disk.** Fsync latency exceeding threshold (say 100ms p99) generates metric alerts. The cause is usually disk degradation; remediation is hardware-level.

* **Query of death.** If a specific query hash is associated with repeated crashes, refuse to execute it (with clear error explaining why). Operators can override after investigation. Not elegant but prevents repeated crashes destroying availability.

* **Schema corruption.** Detection on open. Recovery attempted via redb's internal consistency mechanisms. If unrecoverable, refuse to start, surface clear error, point to backup restoration.

## **Honest tradeoffs**

* **Single-binary simplicity vs modular plugin system.** A plugin system (different storage engines, model providers as plugins) would be useful but adds complexity. Operability trumps flexibility in v1.

* **Configuration surface.** Sketched 40-ish settings; real systems have more. Will grow; principle is hide complexity behind sensible defaults.

* **No managed service in v1.** Customers who want zero ops cannot get it. Terraform modules and Helm chart help but are not a managed product.

* **Limited Windows support.** Server-side only; bet on Linux/macOS being enough. WSL or container available as fallback.

* **Manual scaling.** Operators decide when to scale, what to scale, how. No auto-scaling. Auto-scaling requires v2 distribution or cloud-API hooks.

| Locked Decisions — Operations |
| :---- |
| Single static binary; one process per node; SQLite/Postgres operational simplicity model. Linux x86\_64 \+ aarch64, macOS arm64; Windows deferred. Docker images on Docker Hub and GHCR, multi-arch. Three deployment shapes: embedded library (supported, not optimized), single-node server (primary), single-node with read replicas (v2). CLI tool with admin commands; structured JSON output for scripting. Configuration: TOML file \+ env vars \+ per-table catalog settings \+ runtime SET; sensible defaults throughout. ll admin config validate and ll admin config show \--provenance for confidence. Hot reload via SET for most settings; restart-required settings clearly marked. Upgrade discipline: patch backward-compatible, minor backward-compatible, major with migration tool. Grafana dashboards and Prometheus AlertManager rules shipped with the release. Three log streams (server / slow query / audit) as structured JSON. Helm chart for Kubernetes, Terraform modules for AWS/GCP/Azure. Failure mode handling: disk pressure throttling, memory pressure rejection, external API failure tolerance, query-of-death detection. Documented capacity sizing guidance per workload type. No managed service in v1; self-hosted with operational tools. |

# **Part XVIII — Security**

Security in LL is mostly conventional, with a few specifics that are unique to AI-native systems. The novel parts are how the embedding pipeline interacts with sensitive data, how the multi-modal indexes affect access control, and the model registry security model. The rest is standard authentication, authorization, encryption, and audit.

## **The threat model**

**Threats LL defends against:** unauthorized network access, privilege escalation (authenticated low-privilege user reading data they should not), data theft via storage (someone with disk access — cloud provider, lost backup — reading plaintext), network eavesdropping, credential leakage (API keys and passwords retrievable in cleartext), audit trail tampering, injection attacks.

**Threats LL does not defend against:** root access on the server (if someone has root, the game is over — process can be inspected, memory dumped, keys extracted; standard limit), compromised hardware (TPM-rooted security, attestation — confidential computing territory), DDoS (rate limiting helps, but a determined attacker can overwhelm any single-node system; mitigation lives at the network edge), side-channel attacks (timing attacks on query latency — possible to mount but expensive to fully mitigate; standard databases also do not).

This sets reasonable expectations. LL is a database, not a fortress.

## **Authentication**

**Password-based authentication.** Default for v1. Username and password sent over the wire (must be over TLS). Server validates against \_system.users.password\_hash. Hash function: Argon2id with sensible parameters (memory=64MB, iterations=3, parallelism=4). Argon2 won the Password Hashing Competition for good reason. Password storage: only the hash; never plaintext, even briefly in memory beyond verification. Password requirements: configurable policy; default minimum 12 characters, no specific character class requirements (NIST guidance now favors length over complexity).

**SCRAM-SHA-256.** For Postgres wire compatibility. Implements the SCRAM challenge-response protocol so passwords do not traverse the wire as plaintext.

**API key authentication.** Long random strings (256 bits of entropy, base64url encoded). Sent in connection headers or as the password field with a sentinel username. API keys stored as hashes, not plaintext. The actual key is shown to the user only at creation; the system stores hash(key); verification re-hashes and compares. API keys can have scopes (which tables/operations), expiration, instant revocation by record deletion. Preferred over passwords for non-human users.

**OAuth/OIDC.** Deferred to v2. Server validates JWT tokens from an external identity provider (Auth0, Okta, Google Workspace); tokens carry claims mapped to users and roles.

**mTLS.** Also deferred to v2. Client certificates authenticate the client; subject DN maps to a user.

**Service-to-service tokens.** For LL components and for external embedding model calls. Internal tokens, rotated periodically.

## **Authentication context**

Once authenticated, the session carries a context: user ID and name, role memberships, granted privileges (cached at session start, refreshed periodically), source IP (for audit), authentication method used (for audit and policy decisions). This context flows through every query the session executes.

## **Authorization — RBAC**

The hierarchy: User → Roles → Privileges → Objects. A user belongs to one or more roles. A role has privileges. A privilege is an action (SELECT, INSERT, UPDATE, DELETE, CREATE, ALTER, DROP, EXECUTE) on an object (table, column, model, role). Roles can inherit from other roles, forming a DAG. The admin role typically has all privileges; the readonly role has SELECT on everything; custom roles fill the middle ground.

Privilege enforcement is at planning time, not execution. If you do not have privilege, the query is rejected before any work happens. Optimization: per-session, the user's effective privileges are computed once at session start and cached. Query planning checks the cache, not the catalog tables. Cache invalidated on GRANT/REVOKE — affected sessions reload.

### **Column-level access control**

Some columns are more sensitive than others. LL supports GRANT SELECT (id, title) ON documents TO analyst — analyst can read id and title but not body. The planner enforces by rejecting queries that read forbidden columns or projecting them to NULL. Column-level SELECT grants ship v1; column-level INSERT/UPDATE (which columns can be written) is deferred to v2.

### **Row-level security**

Some users should only see certain rows. Example: tenant isolation in a multi-tenant SaaS — user alice belongs to tenant X and can only see rows where tenant\_id \= X. The mechanism: a policy expression attached to a table; the planner injects the policy expression as an additional predicate into every query against the table. Users cannot bypass it because it is added at planning, not at the user's query.

RLS ships v1. It is critical for multi-tenant applications, and retrofitting it is painful. The cost is more code and more edge cases to test; worth it because multi-tenancy is too important to defer.

### **Object ownership**

Each object has an owner — the user who created it. The owner has full privileges by default and can grant them to others. CREATE TABLE requires CREATE on the database; DROP TABLE requires OWNER or DROP privilege; CREATE MODEL requires CREATE on the model namespace. Standard SQL-style privileges.

## **Encryption at rest**

Three levels of disk encryption: filesystem-level (LUKS, dm-crypt, FileVault), cloud-provider-level (EBS encryption, GCS CMEK, Azure SSE), and application-level (LL encrypts data before writing). For v1, LL relies on filesystem and cloud-provider encryption, not application-level. Application-level encryption is months of careful work; most cloud deployments already encrypt at rest at the provider level; the marginal security benefit of application-level over filesystem encryption is small for typical threat models. v2 can add application-level for customers who need it.

Documented clearly: 'LL relies on filesystem/cloud encryption for data at rest. Application-level encryption is on the roadmap for v2.' Similar to Postgres's stance — TDE offered by enterprise distributions but not core.

### **The catalog secrets exception**

The catalog stores secrets (API keys for embedding models, etc.). These need application-level encryption even if the disk is otherwise plaintext. A master encryption key (MEK) is derived from a passphrase or read from an external KMS at startup. Secrets in the catalog are encrypted with the MEK using AES-256-GCM before storage. The MEK never goes to disk; it lives only in memory during operation. On restart, MEK must be provided.

The MEK passphrase: at startup, the operator provides via interactive prompt, environment variable, file, or KMS integration (AWS KMS, GCP KMS, HashiCorp Vault). The last option is the production recommendation.

## **Encryption in transit**

Wire protocols must support TLS. For v1: native protocol (gRPC) uses standard gRPC TLS — server certificate, optional client certificate (mTLS, v2); strong cipher suites enforced (TLS 1.3 or TLS 1.2 with vetted ciphers as fallback). Postgres wire uses standard Postgres SSL/TLS startup negotiation, TLS 1.2+. Telemetry endpoints are TLS-protected with authentication by default.

Certificate management is the operator's responsibility — LL does not bundle a CA. Standard options: self-signed (dev/internal only), Let's Encrypt (publicly accessible deployments), internal corporate CAs, cloud-provider managed certificates. For internal microservices accessing LL, typically mTLS with internal CA or service mesh handling transit security.

## **Secrets management**

All secrets that LL stores live in the encrypted secrets table, accessed via the SECRET() reference in DDL. Examples: embedding model API keys (OpenAI, Cohere), webhook URLs and authentication, external system credentials, backup destination credentials.

Secrets that LL needs but does not store: the master encryption key (operator-provided per startup), TLS private keys (on disk, protected by filesystem permissions), database user passwords (stored as Argon2 hashes, never decryptable).

DDL for secrets:

CREATE SECRET openai\_key WITH VALUE 'sk-...';

CREATE MODEL openai\_small AS HTTP\_API (  
    endpoint \= 'https://api.openai.com/v1/embeddings',  
    api\_key \= SECRET('openai\_key'),  
    ...  
);

UPDATE SECRET openai\_key WITH VALUE 'sk-new-...';  
DROP SECRET openai\_key;

Secrets cannot be SELECTed back — there is no way to retrieve the decrypted value via SQL. The DDL writes them; only internal code paths reading them for their declared use can decrypt.

## **AI-native security concerns**

**Embedding pipeline talks to external services.** The embedding workers call APIs (OpenAI, etc.) with potentially sensitive text data. Defenses: TLS for all external API calls, API key rotation, per-model audit logging (every embedding call logged with caller, model, timestamp but not the actual text — to avoid the audit log itself being a data exfiltration target), option to mark certain columns as 'do not embed externally' (they must use local models). The last point is genuinely useful for sensitive data; healthcare/financial data should not go to OpenAI's API.

**Vector queries leak query intent.** When a user queries EMBED('confidential project Alpha'), the query text reaches the embedding service. This is information leakage even without storing the data there. The defense is the same: use local models for sensitive workloads. The embedding cache also helps — cache hits do not generate external traffic.

**Model registry as attack surface.** If an attacker can register a malicious model, they can intercept queries or write data. Defense: CREATE MODEL requires elevated privileges (typically admins only). The model is registered with a specific endpoint URL; this cannot be changed without dropping and recreating. Audit log tracks model lifecycle. For paranoid environments: model registration restricted to a whitelist of approved providers.

**Indexed text and vector data is still data.** If sensitive text is indexed in the text index, an attacker with query access can extract terms via probing. Defense: row-level security restricts what data is accessible; column-level encryption (v2) protects text and vectors from being indexed in cleartext.

## **Audit log in depth**

The audit log captures: authentication events (login, logout, failed attempts), authorization events (privilege grants, revocations), schema changes (DDL operations), privilege escalations, model lifecycle, secret lifecycle (without values), data access (configurable per-table — log SELECTs on sensitive tables; not all SELECTs by default), configuration changes.

The audit log is append-only. It can be queried but not modified through normal DDL/DML. For tamper resistance: hash-chained audit log entries (each includes a hash of the previous, making silent deletion detectable) are deferred to v2.

Audit log retention: configurable (default 1 year). Logs older than retention are archived to external storage if configured. Audit log shipping: external SIEMs (Splunk, Datadog) via OpenTelemetry or syslog.

## **Compliance considerations**

LL does not claim compliance certifications; it provides the building blocks customers need to be compliant:

* GDPR — right to erasure (DELETE works; physical deletion via compaction or FORCE REWRITE), encryption at rest (filesystem/cloud-provider in v1), access controls.

* HIPAA — encryption at rest and in transit, access logging, the local-model-only constraint for embedding columns containing PHI.

* SOC 2 — access controls, audit logging, encryption, monitoring.

* PCI-DSS — applicable controls if user stores cardholder data (not specifically built for this but the building blocks apply).

Customers running compliance-sensitive workloads use this combination plus their own deployment hardening to achieve compliance.

## **Honest tradeoffs**

* **Encryption at rest deferred to v2.** Customers in regulated industries needing application-level encryption have to live with filesystem-level or wait for v2. Real cost.

* **Argon2id is slow by design.** Authentication takes \~100ms per attempt. Fine for interactive logins; for high-frequency programmatic access, prefer API keys.

* **RBAC complexity.** Granular permissions are powerful but operators must configure them. Default to closed (deny everything not explicitly granted); document patterns clearly.

* **External embedding API security.** Cannot fully secure data sent to external services. Defense in depth helps but does not eliminate the risk. Customers must understand this.

* **No native HSM integration.** Real production security at scale uses HSMs for the MEK. LL supports reading from external KMS (which can use HSMs internally) but does not directly integrate. Sufficient for v1; v2 may add explicit HSM support.

| Locked Decisions — Security |
| :---- |
| Authentication: Argon2id-hashed passwords, API keys (also hashed), SCRAM-SHA-256 for Postgres wire; OAuth/mTLS deferred to v2. Authorization: RBAC with role inheritance, column-level grants for SELECT, row-level security with policy expressions. Privilege checks at planning time, cached per session. Encryption at rest: filesystem/cloud-provider level for v1; application-level deferred to v2. Catalog secrets encrypted with master encryption key (MEK), AES-256-GCM. MEK provided at startup via passphrase/env/file/KMS integration (KMS recommended for production). Encryption in transit: TLS 1.2+ on both protocols; configurable cipher suites. Secrets management via CREATE SECRET / SECRET() reference; values never readable back. AI-native security: per-model audit logging, local-model-only constraint per column, model registry protection. Comprehensive audit log: authentication, schema, privilege, model, configuration changes. Audit log retention configurable; shipping to external SIEM supported. Hash-chained audit log deferred to v2. Compliance: building blocks (RBAC, RLS, encryption, audit), not certifications. |

# **Part XIX — Distribution Roadmap**

Everything in LL has been single-node-first by deliberate choice. Distribution changes some rules and we need to be explicit about which rules change and which do not. This section is about the v2-and-beyond architectural arc. v1 does not implement distribution. But the design decisions made here constrain v1 — some choices are reversible later, others are not. The point is to know which is which.

## **Why distribution was deferred**

Distribution multiplies engineering cost by approximately 5x. Every layer designed gets harder: storage gets sharding keys; the WAL becomes distributed; transactions become two-phase commit or Paxos; the query planner must reason about network costs; recovery becomes leader election; the catalog becomes a distributed object.

Most use cases do not need it. A single modern node can comfortably hold 5-10TB of data, serve thousands of QPS, and handle hundreds of concurrent connections. That covers the median database workload for years. The percentage of teams that genuinely need more than 10TB of vector data or more than 100K writes per second is a small minority.

Single-node v1 lets LL ship faster, debug easier, and reach the majority of customers. Distribution is the path for customers who grow beyond v1's ceiling. The conservative path is the right one for LL's user base (AI engineers building applications, not hyperscale infrastructure teams).

## **Distribution layers**

Distribution is not a single binary choice. It is a spectrum of capabilities, each addressing different needs:

* **Read replicas (v2).** Copies of the database that handle read queries, syncing from a primary. Adds read scale, geographic read locality, and high availability. Writes still go through a single primary. Most production databases run this way for years without ever needing more.

* **Active-standby (v2).** A hot standby that takes over if the primary fails. Does not add throughput but adds availability. Often combined with read replicas.

* **Sharded writes (v3).** Data partitioned across nodes; each shard owns its slice. Each shard is essentially a single-node LL with its own primary. Writes scale linearly with shards. Cross-shard operations are expensive.

* **Distributed consensus (v3+).** Each shard has multiple replicas using Raft/Paxos for consistency. No single point of failure even within a shard. Most complex; what Spanner-like systems do.

* **Multi-region (v4+).** Shards spanning geographic regions. Adds latency and consistency tradeoffs. Conflict resolution becomes essential.

Each layer is additive on the previous. LL commits to a path through them: v1 single-node only; v2 single-primary \+ read replicas \+ standby for HA; v3 sharded writes with per-shard primary+replicas; v4+ multi-region with explicit consistency models.

This is conservative. It mirrors how Postgres scales (Postgres+Patroni for v2 territory, Citus or YugabyteDB for v3, custom solutions for v4). Most teams stop at v2 and that is fine. The radical alternative — jumping straight to distributed consensus from day one like CockroachDB or FoundationDB — costs dramatically more to build, harder to operate, harder to reason about performance. For LL's user base, the conservative path wins.

## **Read replicas (v2)**

The simplest distribution layer. Most useful per unit of complexity. The model: one primary node accepts writes; one or more replica nodes copy data from the primary; replicas serve reads; optionally, one replica is designated standby (ready for failover).

**What replicates:** every WAL record from primary is shipped to replicas; replicas apply WAL records to their own memtable and storage; files are either received directly from primary or rebuilt from WAL; catalog updates ship via WAL.

**What does not replicate:** buffer pool state (each node warms its own), query cache, embedding job queue (each node processes its own; primary owns the canonical queue).

### **WAL streaming protocol**

The primary streams WAL records to replicas in real-time: primary's WAL writer appends records to its WAL files; replication thread reads new records and pushes them over the network to replicas; replicas append received records to their local WAL; replicas apply records to their state (memtable updates, catalog changes); replicas acknowledge 'I have durably received up to LSN N.'

Three streaming modes: asynchronous (primary does not wait for replica acks — lowest write latency; replicas can fall behind; recent writes may be lost on failover); synchronous (primary waits for at least one replica to acknowledge before commit returns — adds network round-trip \~1ms intra-region, 50-100ms inter-region; guarantees no data loss on failover); quorum-based (primary waits for K of N replicas; balances latency and durability). v2 ships sync and async, configurable per cluster. Default async (matches Postgres default). Sync available for users prioritizing data safety.

### **Replica consistency**

Replicas are eventually consistent with the primary by default. Reads from replicas see a snapshot of the primary as-of some LSN ≤ the primary's current LSN. Read-your-writes is NOT guaranteed across primary/replica. If you write to primary and immediately read from a replica, the replica might not have the write yet.

Two options for clients that need read-your-writes:

* **Read from primary always.** Sacrifices replica read scale. Simplest.

* **Read with LSN target.** Client tracks the LSN at which their write committed; when reading, passes 'read at LSN \>= N.' The replica waits until its applied LSN \>= N before serving the read. Adds latency proportional to replication lag.

Both supported via session settings, similar to MongoDB read preferences.

### **Failover**

If the primary fails, one of the replicas becomes the new primary. Replicas detect primary failure (heartbeat timeout, \~5 seconds), coordinate to elect a new primary (typically the most up-to-date one), the elected replica promotes itself, other replicas reconfigure to follow. Old primary, if it returns, becomes a replica of the new primary (after possible WAL truncation if it has divergent writes).

This requires a coordination layer. v2 uses an external coordinator (etcd or similar). Teams running production databases already have an orchestrator (Kubernetes, Nomad) which provides coordination primitives. Adding etcd is incremental; building consensus into the database itself is months of careful work. LL nodes participate in a coordinated cluster; the coordinator tracks 'who is primary, who is replica, what is their LSN status.' Failover decisions go through the coordinator. Reference deployments for Kubernetes (using its etcd) and for standalone (running etcd alongside LL) ship with v2.

### **Split-brain protection**

The classic distributed system failure: a network partition isolates the primary from replicas. Replicas elect a new primary. Then the network heals and the old primary is still accepting writes. Now you have two primaries; data conflicts; chaos.

Defense: lease-based fencing. Primaries hold a lease from the coordinator that expires periodically. If they cannot renew (due to partition), they step down before the lease expires. New primary cannot be elected until old lease expires. Lease duration configurable; default 10 seconds. Trade-off: shorter lease \= faster failover but more sensitive to network blips.

### **Replica catch-up**

A replica that has been offline must catch up. Brief outage: replay missing WAL records from primary; limited only by WAL retention on primary. Long outage where primary WAL has been truncated: the replica bootstraps. Primary sends a checkpoint (set of files at a known LSN), then WAL from that LSN forward. Essentially 'physical backup \+ WAL replay' performed live. For very long outages or new replicas: ship the latest backup, then catch up via WAL.

## **Sharding for write scale (v3)**

When v2 hits the write throughput ceiling, sharding partitions writes across multiple primaries. Data is divided into N shards based on a sharding key (PK hash or range); each shard is essentially a v2 cluster (primary \+ replicas); the overall cluster has a coordinator that routes queries to the right shard.

What changes: catalog tracks shard assignments; query planner becomes shard-aware; some queries are single-shard (fast), some are cross-shard (slower), some are all-shards (scan-based, expensive); transactions are bounded by shard for performance — cross-shard transactions exist but are expensive.

### **Sharding key selection**

Common approaches:

* **Hash sharding** (shard\_id \= hash(pk) mod N). Evenly distributed; range queries on PK are cross-shard; most queries that filter by PK are single-shard.

* **Range sharding.** Each shard owns a range of PKs. Range queries are single-shard or few-shard. Hot shards possible if data is not uniformly distributed. Rebalancing is harder.

* **Composite sharding.** Hash by one column, then range within. Used by some systems for time-series data.

* **Geo-sharding.** Shard by user/tenant location. Good for geographic locality.

LL ships hash sharding by PK as default with explicit sharding key as an option for advanced users (DISTRIBUTED BY HASH (tenant\_id) etc.). For multi-tenant systems, sharding by tenant\_id ensures each tenant's data is co-located — critical for performance.

### **Cross-shard query challenges**

Some queries are inherently cross-shard. Joins across shards: if documents is sharded by doc\_id and users by user\_id, joining them is expensive. Vector search across shards: top-K vector query must find the K nearest across the entire corpus; querying every shard, getting per-shard top-K, merging globally. Text search across shards: similar. Graph traversal across shards: edges may cross shard boundaries; multi-hop traversals become distributed.

Planning rules: vector search always all-shards (parallel per-shard top-K with merge — cost scales with number of shards but acceptable since each shard is smaller); text search same; graph traversal starts on the shard containing the source node, each hop may cross to other shards; single-shard queries (WHERE includes sharding key) only query the relevant shard. EXPLAIN output in v3 shows which shards a query touches; operators tune sharding keys to maximize single-shard queries.

### **Index sharding behavior**

* **B-tree on sharding key.** Trivially shardable. Each shard has its own B-tree covering its data.

* **Vector index (HNSW).** Each shard builds its own HNSW. Query merges top-K across shards. Quality decreases slightly because HNSW quality is logarithmic in vector count.

* **Text index.** Each shard has its own FST and posting lists. Query intersects across shards.

* **Graph index.** Edges may cross shards. Storage convention: each edge stored once, at the source's shard. Reverse traversal across shards requires querying all shards' reverse indexes. Cross-shard edges add complexity; accepted as the cost of sharded graphs.

### **Cross-shard transactions**

The expensive operation. A transaction touching multiple shards must commit atomically — either all shards see it or none do. Options: two-phase commit (classic, 2 network round trips, in-doubt problem on coordinator failure); Paxos/Raft commit (more failure-resilient, more complex); avoid cross-shard transactions (design data such that transactions fit within one shard — tenant\_id-based sharding helps).

v3 uses 2PC with retry on coordinator failure. Pragmatic choice — well-understood, good enough for the vast majority of workloads. The minority of users with strict requirements can enforce single-shard transactions only.

### **Distributed consensus per shard (v3+)**

Within a shard, HA still matters. v3 ships shards with replication (primary \+ replicas, like v2). v3+ optionally moves to consensus-based replication where each shard is a Raft group. Benefit: no single point of failure within a shard. Cost: more complex implementation, slightly higher latency per operation, requires majority of replicas to be up. v3+ makes this a configuration option per cluster — primary/replica (simpler) or Raft groups (more resilient).

## **Multi-region (v4+)**

Beyond v3, multi-region distribution becomes useful for geographic locality, disaster recovery, and compliance (data residency). The architectural choice: single primary across regions with replicas in other regions (async); multi-primary with conflict resolution; sharded with primaries distributed across regions.

For LL's likely v4 customers (large AI applications with global users), the leading direction is shards distributed by region with replicas in nearby regions. Each shard's primary is in the region where its data is most accessed; replicas in nearby regions for HA and read locality. v4 is years away; the right design depends on what customers actually need. v1-v3 are designed to not foreclose options.

## **What LL deliberately will not do**

* **Eventual consistency by default.** Cassandra-style. Snapshot isolation is LL's default; eventual consistency is opt-in via replicas.

* **Tunable per-row consistency.** DynamoDB-style. LL does not expose this complexity; cluster-level settings.

* **Distributed transactions across arbitrary tables and shards as a routine operation.** 2PC across many shards has poor latency and reliability. Supported but discouraged. Schema design should minimize cross-shard transactions.

* **Hot data movement.** Moving data between shards while live. Useful but complex. v4+ if at all.

* **Geo-replication of full data by default.** Replicating every row to every region. Storage cost is enormous. Done only when explicitly configured per-table.

## **How v1 enables distribution**

**Things that do not change with distribution:** file format (each node's files are local to that node), HNSW page layout (per-file, per-node), buffer pool (per-node), compaction strategy (per-node), query planner internals (extended for distribution but not rewritten), embedding pipeline (per-node, or coordinated for cross-node consistency).

**Things that change:** WAL gets streaming protocol for replicas; catalog gets distributed coordination for schema changes (v2: primary owns catalog, replicas follow); query planner gets shard-aware optimization; transaction commit gets distributed coordination; recovery gets cluster-level (failover protocols); configuration gets cluster-level vs node-level.

**Things v1 must preserve:** stable row IDs (file\_id, offset) cannot change semantically when distribution adds cluster context; logical WAL records must be replayable on a different node; independent file format — each file must be readable without external state; LSN-based MVCC must compose with cluster-wide LSN coordination.

The v1 design preserves these. Distribution-friendly from the start despite being single-node-first. This is the payoff for being deliberate.

## **Migration paths**

* **v1 → v2:** just add replicas. Configure WAL streaming. Existing files remain valid. No data migration. Downtime: minimal (replicas can sync while primary serves traffic).

* **v2 → v3:** sharding requires resharding. A tool reads from v2 cluster, redistributes by sharding key, writes to v3 cluster. Live migration possible with brief downtime; full migration is hours-to-days for large datasets.

These migrations are real engineering efforts but well-defined. The architectural decisions preserve them.

## **Honest tradeoffs**

* **Conservative path means slower scale-out for ambitious customers.** Customers with extreme scale needs may outgrow LL before v3 is ready. They go to CockroachDB or similar.

* **External coordinator dependency.** Requiring etcd (or similar) adds another system to operate. Teams used to self-contained databases see this as a regression. The alternative — implementing consensus ourselves — is worse for v2 timeline.

* **Eventual consistency on replicas is a footgun.** Customers do not always understand the implications. Read-your-writes from replicas requires explicit LSN handling. Real source of bugs in customer apps; documented heavily but some customers still trip on it.

* **Cross-shard query complexity in v3.** Users write queries that look fast in v2 (single-node) and become slow in v3. Clear documentation and operator tools required.

* **Sharding key as a forever decision.** Once chosen, changing it requires full data migration. Users might not pick well at v3 launch and regret it.

* **Multi-region complexity ramps fast.** v4 is where the design becomes truly hard. Different customers want different consistency models. Real risk of over-promising and under-delivering.

| Locked Decisions — Distribution |
| :---- |
| Conservative scaling path: v1 single-node → v2 primary+replicas → v3 sharded → v4 multi-region. v2: WAL streaming for replication, sync/async/quorum modes; external coordinator (etcd) for failover. Read replicas with eventually-consistent reads; explicit LSN-based read targeting available. Lease-based primary fencing to prevent split-brain. v3: hash sharding by configurable key (default PK); per-shard primary/replica structure. Cross-shard queries always supported; performance is honest with operator. Vector/text searches naturally distributable as per-shard top-K with merge. Graph storage by source shard; cross-shard reverse traversal accepted cost. 2PC for cross-shard transactions; encourage single-shard transaction design. Optional Raft-based replication per shard for v3+ (configurable per-cluster). v4+ multi-region: shards distributed by region, replicas for HA and read locality. Reject: default eventual consistency, per-row consistency tuning, hot data movement in v3 era. v1 preserves the structural primitives (row IDs, logical WAL, file independence, LSN-based MVCC) that enable distribution. Migration tooling for v1→v2 (trivial) and v2→v3 (resharding). |

# **Part XX — Comparison with OriginChain v1**

This section exists because the design of LL is only meaningful in contrast to what came before. OriginChain v1 was a real attempt at the same problem space. Naming what changed and why makes the design intentional rather than emergent.

## **What OriginChain v1 actually was**

A vector database with a hashmap of relationships. Functionally: pgvector with extra metadata. Architecturally: a federation of subsystems with application-level glue. The patents, the brand work, the GTM motion, the NVIDIA Inception co-branding, the website, the architecture diagrams — all sat on top of a substrate that was, at the storage layer, conventional. The 'AI-native managed database' promise did not deliver at the layer that matters most: bytes on disk.

The website inconsistencies that the brand audit surfaced — conflicting graph algorithm counts, conflicting vector latency numbers, inconsistent PITR naming, conflicting feature claims across product pages — were not marketing problems. They were symptoms of an architecture that could not make definite performance claims because the architecture was not crisp enough to support them. A federated system cannot honestly claim 'vector search latency is X' because vector search performance depends on whether the related metadata is in the same store, whether the filter happens before or after the vector lookup, whether the graph traversal is in another system entirely. The numbers fragment because the substrate fragments.

## **Where LL differs structurally**

### **One storage substrate, not four**

OriginChain used (or would have used at scale) one system for vectors, another for full-text, another for relational, another for graph. Even when wrapped in a managed endpoint, the underlying data lived in separate stores with separate consistency models. LL has one file format. A row, its vector, its text tokens, and its edges all land in one WAL entry, get versioned together, compact together. A failed embed rolls back the row. This is a different category of system.

### **One query plan, not application-level joining**

OriginChain's 'SQL, vector, full-text, graph, and natural language queries on a single managed endpoint' was, when traced through, a router that dispatched to subsystems and merged results in the application layer. LL has a cost-based optimizer that pushes predicates across modalities. The planner can decide that a date filter is more selective than a vector search and pre-filter before the ANN lookup. OriginChain could not make that decision because the planner did not span the modalities.

### **MVCC across all index types, not eventual consistency**

OriginChain's vector index would have had near-real-time visibility — Elasticsearch-style 1-second lag, or worse. A user who inserted a document and immediately searched for it would miss it. LL has read-your-writes across all four access patterns because the memtable carries in-memory analogs of all four index types. That is a real correctness improvement for agentic workflows where the agent writes and queries in the same turn.

### **A cost model that estimates filtered ANN cardinality**

Per-file centroid distance distributions enable the optimizer to choose between pre-filter, post-filter, and integrated-filter strategies. OriginChain could not do this because OriginChain did not have a cost model worth the name. Most vector databases do not. Filtered vector search is where the entire vector database industry quietly falls over, and LL has a principled answer to it. This is the single most undervalued piece of work in the LL design.

### **Embeddings as first-class schema features, not user data**

In OriginChain, the user generated embeddings client-side and inserted them as vectors. In LL, the user declares EMBED FROM body USING openai\_small and the system owns the lifecycle — generation, caching, regeneration on updates, model migration with dual-read. This is a fundamentally different contract with the user, and it is the contract that 'AI-native' actually means.

## **What OriginChain had that LL does not yet**

LL inherits but does not replicate OriginChain's product apparatus. The brand work, the positioning, the GTM motion, the patent portfolio, the NVIDIA Inception relationship, the customer conversations — all of that is real value that does not reset. If LL replaces OriginChain as the technical substrate, the surrounding apparatus translates.

OriginChain forced the confrontation with a specific category of problem — a managed endpoint for multi-modal queries — at the product level. That product instinct was correct even if the implementation was unsatisfying. LL is what OriginChain should have been technically.

## **The honest summary**

*LL is the system OriginChain was trying to be. The novelty is not in any single component — every individual technique has prior art. The novelty is the integration. A unified storage substrate that does relational, vector, text, and graph access in one file format under one WAL under one MVCC model under one cost-based optimizer is not a thing anyone has shipped.*

If LL gets built well, it is a legitimate frontier of database systems. The work is hard. The timeline is long. The risk profile is different from OriginChain's. But the unsatisfying feeling of OriginChain v1 — the sense that it was less than what it claimed to be — does not need to attach to LL. LL is what we said it would be.

# **Part XXI — Version Roadmap**

This section makes the version split unambiguous. Every locked decision elsewhere in this document belongs to a specific version. The complete picture is below.

## **v1 scope — the shippable single-node engine**

v1 is the standalone single-node engine that delivers on the integration thesis. It is not feature-complete relative to mature databases (PostgreSQL has 25+ years of feature growth on it) but it is architecturally complete for AI-native workloads.

### **v1 includes**

* Single-node engine; one static binary.

* Physical record with stable row IDs, columns including scalars, vectors, text, edges; xmin/xmax MVCC fields.

* Memtable: 256MB default configurable, with skiplist row store, in-memory flat vector index, in-memory inverted index, bidirectional edge hashmaps; up to 4 frozen memtables in flight.

* File format: PAX-layout column chunks, embedded HNSW (int8-quantized \+ full-precision rerank, BFS-reordered, node-major), text section (FST \+ PFOR-Delta posting lists with optional positions, score-augmented top-K lists), edge section (forward \+ reverse CSR with columnar properties, per-file bloom filters), FlatBuffers footer with checksums and per-page zone maps.

* Compaction: tiered 4-tier with 8x ratio, HNSW rebuild at tier-1+ with merge at tier-0→tier-1, all other indexes rebuilt every compaction, MVCC reconciliation, three triggers, atomic catalog swap.

* Buffer pool: W-TinyLFU eviction, 64-shard, two-tier with pinned region for footers/FST roots/HNSW entry points, direct I/O via io\_uring on Linux, batched file-open warming.

* Catalog: redb-backed, fully cached in memory, schema evolution with column\_id stability, model registry as first-class objects.

* Query layer: SQL with vector/text/graph extensions, logical plan IR with first-class multi-modal operators, physical plan with cost-based selection, three filtered vector search strategies, time-based cost model with per-file vector centroid distributions for selectivity estimation.

* Execution engine: morsel-driven via DataFusion \+ Arrow, tokio for I/O and rayon for CPU, per-query memory budgets, multi-modal operators with internal parallelism, cancel tokens.

* Transactions: Snapshot Isolation primary, Read Committed available, optimistic concurrency with first-committer-wins, LSN-based snapshots, write-set tracking.

* Embeddings: deferred default with inline mode opt-in, system-table job queue, per-model batching and concurrency, embedding cache per-model, dual-read migration model.

* Wire protocols: gRPC native protocol with Arrow IPC streaming, partial Postgres wire compatibility (simple \+ extended query protocols, transaction control, prepared statements).

* SDKs: Rust client as foundation; SQL-based Python and TypeScript clients in v1, full DSL fluent API targeting v1.5+.

* Authentication: Argon2id passwords, API keys, SCRAM-SHA-256 for Postgres wire.

* Authorization: RBAC with role inheritance, column-level SELECT grants, row-level security with policy expressions.

* Encryption: filesystem/cloud-provider for data at rest; TLS 1.2+ for in transit; AES-256-GCM for catalog secrets with KMS-rooted MEK.

* Observability: rich EXPLAIN with estimated-vs-actual and strategy choice, per-query telemetry with multi-modal stats, slow query log, query history aggregation, Prometheus metrics, OpenTelemetry tracing, comprehensive \_system.\* tables.

* Recovery and durability: WAL with group commit, three-tier checksums, checkpoints, hot backup, configurable durability levels, PITR with 7-day default WAL retention.

* Operations: single-binary deployment, multi-arch Docker, Kubernetes Helm chart, Terraform modules for AWS/GCP/Azure, comprehensive CLI, Grafana dashboards and AlertManager rules, graceful resource-pressure handling.

* Audit log for security and compliance events.

### **v1 excludes (deferred)**

* Distribution in any form (read replicas, sharding, multi-region) — all v2+.

* Application-level encryption at rest — v2.

* OAuth/OIDC and mTLS authentication — v2.

* Hash-chained audit log — v2.

* Adaptive execution (mid-query plan changes based on actual cardinality) — v2.

* Native Python DSL with plan-building (v1 wraps SQL) — v1.5+.

* Logical backup (SQL dump) — v2; v1 ships physical backup only.

* PostgreSQL COPY protocol, replication protocol, full PL/pgSQL — deferred.

* Column-level INSERT/UPDATE grants (column-level SELECT grants ship v1) — v2.

* Transactional DDL (DDL auto-commits in v1) — v2.

* Window functions, advanced analytical SQL features beyond core — incremental.

* Cold-start warmup (snapshot W-TinyLFU keys to disk and reload at startup) — v2.

* NUMA-awareness for buffer pool on multi-socket machines — v2.

* Adaptive morsel sizing — v2.

* Score-augmented posting list maintenance during compaction — v2 optimization.

* Product Quantization for HNSW vectors (scalar int8 quantization ships v1) — v2.

* PostgreSQL pg\_stat\_statements wire compatibility — v2 (LL ships its own \_system.query\_stats).

* Managed service offering — product strategy decision.

## **v2 scope — read replicas, HA, fillout**

* Read replicas with WAL streaming (sync/async/quorum modes).

* Hot standby with automatic failover via external coordinator (etcd).

* Lease-based primary fencing to prevent split-brain.

* LSN-based read targeting for read-your-writes from replicas.

* Application-level encryption at rest with KMS integration.

* OAuth/OIDC and mTLS authentication.

* Hash-chained audit log.

* Adaptive execution: plans adjust mid-flight based on actual cardinality.

* Native Python DSL with plan-building (bypasses SQL parser).

* Logical backup (SQL dump) and pg\_dump-compatible output.

* Column-level INSERT/UPDATE grants.

* Transactional DDL.

* Cold-start warmup.

* NUMA-aware buffer pool.

* Product Quantization for HNSW vectors.

* Score-augmented posting list maintenance during compaction.

* Learned-to-rank fusion training from query logs.

* SSI (Serializable Snapshot Isolation) as an option.

## **v3 scope — sharded writes**

* Hash sharding by configurable key (default PK).

* Per-shard primary/replica structure.

* Optional Raft-based replication per shard.

* Shard-aware query planner with single-shard / cross-shard / all-shards classification.

* 2PC for cross-shard transactions.

* Cross-shard vector and text search via parallel per-shard top-K \+ merge.

* Cross-shard graph traversal.

* Resharding tool for v2 → v3 migration.

* Hot shard rebalancing (limited).

## **v4+ scope — multi-region**

* Shards distributed by region with replicas in nearby regions.

* Per-region primaries with cross-region replication.

* Explicit consistency models for multi-region reads and writes.

* Conflict resolution for multi-primary configurations (if supported).

* Geographic policy enforcement (data residency for GDPR etc.).

## **Build cost**

LL is hard. A realistic estimate is 2-3 years of focused work with a small competent team, or 4-5 years solo. The file format alone is months of careful design and testing. The cost model is research-grade work. The query planner extensions to DataFusion are substantial. The embedding lifecycle is its own product subsystem. v1 is not a wrapper over existing infrastructure; it is a database in the literal sense.

v2 adds approximately 6-9 months on top of v1. v3 adds another 12-18 months. v4+ is open-ended depending on customer demand and consistency model choices.

The premium for doing the integrated work nobody has done is paid in engineering time. The reward is a system that is genuinely differentiated rather than another vector-database-plus-bolt-ons.

# **Closing**

## **What this document is**

This is the canonical architectural reference for LL produced through a complete bottom-up design conversation. It is not the build manual — it does not specify file layouts at the byte level, function signatures, error code values, or test plans. It is the design that those things should implement. The intent is that an engineer reading this document understands what LL is and why each major decision was made, and could begin implementation without having to re-derive the architecture.

## **How to use this document**

**During design reviews:** the locked-decision boxes are the contract. Decisions in those boxes should not be reopened casually; they have downstream commitments. Decisions stated in prose but not locked are subject to refinement as implementation surfaces details.

**During implementation:** each part of this document maps to a part of the system. A team implementing the file format reads Parts V-VIII. A team implementing the query layer reads Parts IX-XI. The locked decisions at the end of each part are the implementation acceptance criteria.

**During iteration:** when a v1 decision must be revisited, update both the prose and the locked-decision box, and note the date and reason. The document is meant to stay current; stale design documents create more confusion than they resolve.

## **Decisions outside this document**

Several decisions are deliberately not in this document because they belong to product strategy, not architecture:

* The actual name of the project (LL is a working code-name).

* Commercial licensing (Apache 2.0, BSL, proprietary, etc.).

* Open-source release timing and scope.

* Managed service strategy.

* Pricing model.

* Initial customer profile and GTM motion.

* Team structure and hiring plan.

* Specific deadlines for v1, v2, v3.

These are real decisions that need to be made, but they are downstream of the architecture, not constraints on it.

## **The single line**

*LL is a single-node AI-native database that integrates relational, vector, full-text, and graph storage into one storage substrate, one WAL, one MVCC model, one cost-based optimizer, and one consistent file format. It is what OriginChain v1 was trying to be.*

