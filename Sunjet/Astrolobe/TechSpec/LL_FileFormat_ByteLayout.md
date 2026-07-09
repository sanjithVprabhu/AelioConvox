# VSS File Format — Byte-Level Layout (v0, format_version = 1)

VSS — *Versioned Storage Substrate* — is LL's on-disk file format (extension `.vss`).
Implementation-grade spec for the immutable VSS data file (`.vss`). One file = one
row group (memtable flush output); immutable after rename. Covers all four modalities:
scalar/vector/text/edge columns + embedded HNSW, text (FST + PFOR-Delta), and edge
(forward/reverse CSR) indexes.

Authority: this refines Part V of `LL_Architectural_Specification.md` and obeys
`LL_Decisions_Delta.md` D-001 (three-layer row identity). Where they conflict, the
Delta wins, then this doc, then the spec.

Scope-out (separate steps): WAL record format, memtable structures, compaction
algorithm, catalog persistence. This doc is purely the on-disk artifact and its reader.

---

## 0. Conventions

- **Endianness:** little-endian everywhere. No byte-swap on x86/ARM.
- **Alignment:** every top-level region starts on an 8-byte boundary. Vector data is
  16-byte aligned (AVX-512 friendly; supersedes the spec's 8-byte note).
- **Integers:** fixed-width `u8/u16/u32/u64`/`i*` as named. "varint" = LEB128 unsigned
  where stated.
- **Checksums:** CRC32C (Castagnoli, hardware `crc32` instruction). Three layers:
  per-page, per-section, footer. Each computed over the *compressed/on-disk* bytes of
  its unit, excluding the checksum field itself.
- **Compression:** per-page. `0=none, 1=zstd(level 3 default), 2=lz4`. Trained ZSTD
  dictionaries (optional, from compaction) referenced from the footer.
- **Footer encoding:** FlatBuffers (zero-copy). Everything else is raw bytes.
- **Row identity (D-001, D-005, D-006):** three ID layers.
  - `global_row_id: u64` — the only stable-forever identity (assigned at WAL time).
    Edge targets and all cross-file references store this.
  - `local_offset: u32` — dense row position `0..row_count` within this file. Column
    chunks, MVCC columns, text posting doc IDs, and CSR *source* indexing all use it.
  - `hnsw_node_id: u32` — **internal to one HNSW section**, BFS-reordered for graph
    locality. Drives `page_id = hnsw_node_id / nodes_per_page`. A node slot stores its
    `local_offset` (so rerank/MVCC can fetch the row); neighbor lists store
    `hnsw_node_id`s. This split is required because BFS reordering must reorder *node
    IDs* without disturbing row order (multiple vector columns each want their own
    order; text/edge/scalar want row-order locality).
  - The `local_offset -> global_row_id` map is a **separate TranslationTable section**
    (not inlined in the footer — see §5), referenced by the footer.

### File regions, in order
```
+----------------------------------------------------------+
| [0]   File preamble            (fixed 64 bytes)          |
| [1]   Column-chunk region      (one chunk per column)    |
| [2]   Embedded-index region    (HNSW / text / edge)      |
| [3]   Optimizer-stats region   (centroid distributions)  |
| [4]   TranslationTable section (local_offset->global u64)|
| [5]   Footer (FlatBuffers)     (table of contents)       |
| [6]   Footer trailer           (fixed 16 bytes)          |
+----------------------------------------------------------+
```
Read path: seek `end-16`, read trailer, validate `end_magic`, read footer length,
read+CRC the footer, then everything is addressable by absolute file offset.

---

## 1. File preamble (fixed 64 bytes, offset 0)

Identification/sanity only; the footer is authoritative.

| off | size | field                | notes |
|----:|-----:|----------------------|-------|
| 0   | 4    | `magic`              | on-disk bytes `VSS1` (`u32` 0x31535356) |
| 4   | 2    | `format_version`     | `1` |
| 6   | 2    | `preamble_flags`     | bit0 = little-endian (always 1 in v1) |
| 8   | 16   | `file_uuid`          | random 128-bit |
| 24  | 8    | `min_lsn`            | min LSN of rows in file |
| 32  | 8    | `max_lsn`            | max LSN of rows in file |
| 40  | 8    | `row_count`          | N (local offsets are `0..N`) |
| 48  | 8    | `schema_fingerprint` | xxh3-64 of the schema as written |
| 56  | 8    | `creation_unix_nanos`| wall clock at write |

Column-chunk region begins at offset 64.

---

## 2. Column-chunk region

One **chunk** per column, in schema column order. Includes two system columns
(`xmin`, `xmax`) marked `is_system`. MVCC `xmax = u64::MAX` means ∞ (live).

### 2.1 Chunk header (variable; located via footer SectionDirectory)
| size | field | notes |
|-----:|-------|-------|
| 4 | `column_id`        | stable catalog ID (D-001 namespace; system cols flagged) |
| 2 | `logical_type`     | enum: 0=Bool,1=I8…,10=F32,11=F64,20=Utf8,30=Vector,40=Edge,50=Timestamp,… |
| 1 | `encoding`         | 0=plain,1=dict,2=rle,3=delta,4=for (frame-of-reference) |
| 1 | `compression`      | 0/1/2 |
| 2 | `chunk_flags`      | bit0 nullable, bit1 dict_present, bit2 quantized(vector), bit3 is_system |
| 2 | `vector_quant`     | 0=none(f32),1=fp16,2=int8 (vector cols only) |
| 2 | `vector_dim`       | (vector cols only; else 0) |
| 8 | `value_count`      | usually == row_count |
| 8 | `null_count`       | |
| 4 | `num_pages`        | |
| 8 | `dictionary_offset`| absolute; 0 if none |
| 8 | `dictionary_len`   | |
| 8 | `page_index_offset`| absolute offset of the page-index array |
| 8 | `total_bytes`      | on-disk size of all pages |

Per-column min/max/NDV/histogram live in the footer zone maps, not here (keeps the
header lean and the optimizer stats in one place).

### 2.2 Page index array (one entry per page)
| size | field |
|-----:|-------|
| 8 | `page_offset` (absolute) |
| 4 | `compressed_len` |
| 4 | `uncompressed_len` |
| 4 | `row_count` (rows in page) |
| 4 | `first_local_offset` |

Pages are 8KB–1MB (column data). The page is the unit of decompression → parallel
decode. `local_offset` of a row = `first_local_offset + index_within_page`.

### 2.3 Page header (prepended to each page payload)
| size | field |
|-----:|-------|
| 1 | `page_type` (0=column,1=hnsw,2=postings,3=csr,…) |
| 1 | `compression` |
| 2 | `reserved` |
| 4 | `uncompressed_len` |
| 4 | `compressed_len` |
| 4 | `page_crc32` (CRC32C over the compressed payload bytes) |

Then `compressed_len` bytes of payload. After decompression, decode per §2.4.

### 2.4 Per-type page payloads (post-decompression)
- **Scalar** — array of `row_count` values in `encoding`:
  - `plain`: packed fixed-width values.
  - `dict`: `u32` indices into the chunk dictionary (offsets+bytes blob).
  - `rle`: `(value, run_len:varint)*`.
  - `delta`: zig-zag deltas (good for timestamps, `xmin`).
  - `for`: `base:u64` + bit-packed offsets at `bit_width:u8`.
  - Nulls (if `nullable`): a leading validity bitmap (`ceil(row_count/8)` bytes), Arrow-compatible.
- **Vector** — contiguous, 16-byte aligned. `f32[dim]` per row by default; `fp16[dim]`
  or `int8[dim]` (+per-page `scale:f32, zero:i8` for int8) if `vector_quant != 0`.
  Full precision lives here; the HNSW page holds a separate int8 copy for traversal.
- **Text payload** — the raw strings: `offsets: u32[row_count+1]` + `bytes` (zstd at
  page level; dict-encoded if low-cardinality). The inverted index is in §3.2, not here.
- **Edge inline stub** — per row: `count: varint`. If `count <= 8`: `count` × inline
  `target_global_row_id: u64`. Else: `csr_ptr: u64` (offset into the §3.3 forward CSR
  slice for this source). Low-degree nodes (power-law majority) read edges in one page.

---

## 3. Embedded-index region

One section per indexed column. Each section: a **section header** (with own CRC32C
over the section body), then the index payload. Located via footer SectionDirectory.

### 3.1 HNSW section (per vector column)
**Refinement vs spec (D-006):** node IDs live in their own BFS-reordered space
(`hnsw_node_id`), distinct from `local_offset`. To preserve O(1) page lookup
(`page_id = hnsw_node_id / nodes_per_page`), node slots are **fixed-size** and hold
only level-0 adjacency + the quantized vector + the node's `local_offset`. Upper-layer
adjacency (small, ~6%) lives in a separate compact blob at section end, consulted only
during the rare upper-layer descent. A build-time-only reverse map
(`local_offset -> hnsw_node_id`) is used while constructing the graph; it is not needed
at read time.

**v0 implementation note (ll-index `page.rs`):** slots are packed flat (slot `i` at
`pages_off + i*slot_size`), which preserves O(1) lookup; the 4096-byte page *grouping* is
a disk-IO optimization deferred with the buffer pool. Per-dimension 8-bit quant params
(`min[dim]`, `scale[dim]`) are stored in a quant block right after the section header
(the spec's per-page single scale was for the column chunk; HNSW uses per-dimension for
better recall). No internal section CRC — the framing layer's `SectionEntry.crc32` covers
the section once embedded in a `.vss` file.

Section header:
| size | field |
|-----:|-------|
| 4 | `column_id` |
| 2 | `vector_dim` |
| 2 | `M` (max neighbors/upper layer) ; `M0 = 2*M` for layer 0 |
| 2 | `ef_construction` |
| 1 | `quant` (1=int8) |
| 1 | `num_levels` |
| 4 | `entry_point_hnsw_node_id` |
| 4 | `nodes_per_page` |
| 4 | `num_pages` |
| 8 | `upper_blob_offset`, 8 `upper_blob_len` |
| 4 | `section_crc32` |
| 4 | `recall_at_10_estimate_milli` (×1000; surfaced in EXPLAIN) |

HNSW pages: fixed `page_size` = smallest multiple of 4096 that fits ≥2 node slots.
Slot at index `i` within the section holds the node with `hnsw_node_id == i`
(`page_id = i / nodes_per_page`, `slot_in_page = i % nodes_per_page`).
Node slot (fixed):
| size | field |
|-----:|-------|
| 4 | `local_offset` (row position; for rerank/MVCC fetch) |
| 1 | `level` |
| 1 | `l0_count` |
| 2 | pad |
| 4×M0 | `layer0_neighbors: u32[M0]` (hnsw_node_ids; unused = `u32::MAX`) |
| dim | `qvec: int8[dim]` |

Upper-layers blob: for each node with `level>0`, `(hnsw_node_id:u32, level:u8,
per-level counts + neighbor u32 arrays of hnsw_node_ids)`. Indexed by a small sorted
`hnsw_node_id` table at blob start for binary search.

Search: descend upper blob from `entry_point_hnsw_node_id` → layer-0 beam over pages
(page-local, BFS-reordered in hnsw_node_id space for locality) using int8 distances →
top candidate `hnsw_node_id`s → read each slot's `local_offset` → fetch full f32
vectors from §2.4 column chunk by `local_offset` → exact rerank → MVCC filter via the
`xmin/xmax` columns (also indexed by `local_offset`). Results map to `global_row_id`
via the TranslationTable section. Deletions handled at rerank (immutable pages).

### 3.2 Text section (per text column)
Section header:
| size | field |
|-----:|-------|
| 4 | `column_id` |
| 1 | `positions_present` |
| 8 | `num_terms` |
| 8 | `fst_offset`, 8 `fst_len` |
| 8 | `termmeta_offset`, 8 `termmeta_len` |
| 8 | `postings_offset`, 8 `postings_len` |
| 8 | `positions_offset`, 8 `positions_len` (0 if absent) |
| 4 | `section_crc32` |

- **FST** (BurntSushi `fst` crate bytes): term → `u64` output = index into the
  term-meta array. Prefix/suffix sharing; prefix & range queries free; mmap-able.
- **Term-meta array** (`num_terms` entries):
  `posting_offset:u64, doc_freq:u32, posting_blocks:u32, skip_offset:u32,
   aux_offset:u32 (0 if none)`.
- **Postings region**: per term, PFOR-Delta blocks of 128 local doc IDs.
  Block: `count:u16, bit_width:u8, num_exceptions:u8, exceptions:(pos:u8,val:u32)*,
  packed_deltas`. Frequencies bit-packed after deltas (omitted if all == 1).
  **Skip list** every 128 entries: `(last_doc_local:u32, block_offset:u32)` for
  logarithmic AND/OR intersection.
- **Score-augmented aux** (terms with `doc_freq > 10_000`): `top-1000
  (doc_local:u32, impact_q:u16)` by BM25 contribution. Top-K text queries hit this
  first; main postings untouched if K confident answers return.
- Doc IDs are **local u32**; translate to global via footer when merging across files.
- Positions (opt-in): per (term, doc) varint position lists in the positions region.

### 3.3 Edge section (per edge column)
Section header:
| size | field |
|-----:|-------|
| 4 | `column_id` |
| 1 | `reverse_present` |
| 4 | `num_properties` |
| 8 | `num_edges` |
| 8 | `base_lsn` (for compressed per-edge MVCC) |
| 8 | `fwd_offsets_off`, 8 `fwd_targets_off`, 8 `fwd_len` |
| 8 | `rev_off`, 8 `rev_len` |
| 8 | `props_off`, 8 `props_len` |
| 8 | `bloom_off`, 8 `bloom_len` |
| 8 | `mvcc_off`, 8 `mvcc_len` |
| 4 | `section_crc32` |

- **Forward CSR**: `offsets: u32[row_count+1]` indexed by local source offset;
  `targets: u64[num_edges]` = **global_row_id** (D-001), sorted within each source,
  PFOR-Delta encoded. Targets are global because edges cross files and survive
  compaction.
- **Property arrays**: parallel to `targets`, one columnar array per declared property
  (delta for timestamps, dict for labels, FOR for small ints, raw f32). Unqueried
  properties never read.
- **Reverse index** (if `reverse_present`): sorted `targets_distinct: u64[]` (global),
  parallel `source_ptr_lists` (local source offsets), fanout index every 128 targets
  for binary search.
- **Bloom filter** over target global IDs (~1KB/file): reverse traversal of T checks
  every file's bloom first; only "maybe" files consult their reverse index.
- **Compressed per-edge MVCC**: `base_lsn` in header; per-edge `xmin` as PFOR-Delta
  small delta (1–2 B); per-edge `xmax` sparse sidecar `(edge_index:u32, xmax:u64)*`
  present only for deleted edges. Typical 1–3 B/edge of version metadata.

---

## 4. Optimizer-stats region

Bulkier stats kept out of the FlatBuffers footer and referenced from it.
- **Per-vector-column centroid distance distributions** (the cost-model differentiator):
  ~100 sampled centroids; per centroid a distance histogram (e.g., 64 buckets `f32`
  edges + `u32` counts). ~100 KB/file. Used to estimate filtered-ANN selectivity.
- **Per-scalar-column histograms**: equi-height ~256 buckets + most-common-values list.

---

## 5. Footer (FlatBuffers) — table of contents

```fbs
// footer.fbs  (format_version 1)
namespace ll.format;

enum SectionType : ubyte { ColumnChunk, Hnsw, Text, Edge, OptimizerStats, TranslationTable }
enum SectionEncoding : ubyte { Plain, Delta, Pfor }  // for TranslationTable & others

struct SectionEntry { type:SectionType; encoding:SectionEncoding; column_id:uint; offset:ulong; length:ulong; crc32:uint; }

table ZoneMap {            // per column, per page
  column_id:uint; page_index:uint;
  min:[ubyte]; max:[ubyte];        // typed, little-endian bytes
  null_count:ulong; distinct_estimate:ulong; total:ulong;
  centroid:[float];  max_radius:float;   // vector columns only
}

table BloomEntry { column_id:uint; offset:ulong; length:ulong; }

table CompressionDict { column_id:uint; offset:ulong; length:ulong; }

table MvccSummary { min_xmin:ulong; max_xmax:ulong; tombstone_count:ulong; live_count:ulong; }

table Footer {
  file_uuid:[ubyte];                 // 16, mirrors preamble
  format_version:ushort;
  min_lsn:ulong; max_lsn:ulong;
  row_count:ulong;
  schema_fingerprint:ulong;
  schema_blob:[ubyte];               // full serialized schema as-written (evolution)
  sections:[SectionEntry];           // includes the TranslationTable section entry
  zone_maps:[ZoneMap];
  blooms:[BloomEntry];
  compression_dicts:[CompressionDict];
  mvcc_summary:MvccSummary;
  writer_version:string;
}
root_type Footer;
```

Notes:
- **Translation table is a separate section** (`SectionType.TranslationTable`), not
  inlined here (D-005). The footer only references it via its `SectionEntry`. Body is
  `global_row_id: u64[row_count]` indexed by `local_offset`. v0 encoding `Plain`; later
  `Delta`/`Pfor` (global IDs within a file are near-monotonic → ~1 B/row). The buffer
  pool pins it for small files and lazily loads it for large tier files. global→local
  (rare) is a binary search over a sorted copy built on load.
- `schema_blob` enables online schema evolution (add col → null/default for old files;
  drop col → ignore; widen type → cast on read), gated by `schema_fingerprint` equality
  fast-path.
- Zone maps drive file/page pruning; vector `centroid + max_radius` drive approximate
  vector pruning; centroid *distributions* (the richer stat) live in §4.

---

## 6. Footer trailer (fixed 16 bytes, at end of file)

| order | size | field |
|------:|-----:|-------|
| 1 | 4 | `footer_crc32` — CRC32C over the footer FlatBuffers bytes |
| 2 | 8 | `footer_len` — length in bytes of the footer FlatBuffers blob |
| 3 | 4 | `end_magic` — on-disk bytes `VSS1` (`u32` 0x31535356) |

Reader: read final 16 B → check `end_magic` → `footer_len` → footer blob at
`[end-16-footer_len, end-16)` → verify `footer_crc32`. (Spec said `u32` footer length;
widened to `u64` to allow large translation tables.)

---

## 7. Three-layer checksum model

1. **Page** — `page_crc32` in each page header (validate on page access).
2. **Section** — `section_crc32` in each index section header (validate lazily on
   section open).
3. **Footer** — `footer_crc32` in the trailer (validate on file open).

CRC32C is hardware-accelerated; microseconds per page. A failed page CRC surfaces a
clear corruption error and marks the file suspect rather than returning bad data.

---

## 8. Implementation order for this file format (Step 1)

1. ✅ Preamble + footer + trailer round-trip (write → read → verify CRCs) with an empty
   file. Establishes the skeleton and the footer schema.
2. ✅ Scalar column chunks (plain + nulls), per-page zone maps, translation table.
   (Still TODO: dict/RLE/delta/FOR encodings behind the `encoding` byte; per-page ZSTD
   compression — v0 pages are stored uncompressed.)
3. ✅ Vector column chunks (f32, nullable) with per-page centroid + max-radius zone
   maps. (Deviations to revisit: on-disk 16-byte vector alignment is NOT yet enforced —
   v0 copies into an aligned `Vec<f32>` on read, so strict on-disk padding is deferred
   to the mmap/buffer-pool milestone. Column-level int8/fp16 quantization is deferred;
   int8 is implemented where it's actually needed — the HNSW section — so the column
   chunk keeps full precision for exact rerank.)
4. HNSW section: build from a flat vector set, fixed-slot pages, upper blob, rerank.
5. Text section: FST + PFOR-Delta postings + skip lists + score-aug aux.
6. Edge section: forward CSR + properties, then reverse + bloom, then compressed MVCC.
7. Optimizer-stats region (centroid distributions, histograms).

Each step ships with: a property-based round-trip test (write arbitrary data → read →
assert equality) and a corruption test (flip a byte → expect the right CRC layer to
catch it). This is the seed of the deterministic-simulation discipline (D-004).
```
