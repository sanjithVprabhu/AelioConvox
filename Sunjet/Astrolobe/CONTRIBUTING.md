# Contributing to LL

Thanks for your interest. LL is an AI-native database that puts relational, vector,
full-text, and graph data in one engine. This guide covers how to build it, run the tests,
and submit a change.

## Prerequisites

- **Rust** (stable, edition 2021) — install via [rustup](https://rustup.rs).
- That's it for the core engine. It has no runtime dependencies and no daemon.
- The head-to-head benchmark *runners* for other systems additionally need Docker and Python
  (`pgvector_bench.py`, `qdrant_bench.py`); LL's own benchmark does not.

## Build & test

```sh
cargo build --workspace            # build everything
cargo test                         # run the full test suite
cargo clippy --workspace --all-targets   # lints (CI treats warnings as failures)
cargo fmt --all                    # format before committing
```

The suite includes per-feature unit tests, cross-source MVCC tests, crash-recovery tests, a
deterministic model-based simulation (`crates/ll-engine/tests/sim.rs`), and read-path fuzzing.
All of it should pass on a clean checkout with no setup.

To see the engine end-to-end:

```sh
cargo run --release -p ll-query --example wedge   # one plan over all four modalities
cargo run --release -p ll-bench -- ./benchdata 50000 128 50 16   # filtered-vector benchmark
```

## Workspace layout

LL is a Cargo workspace; each crate owns one layer of the engine:

| Crate | Responsibility |
|---|---|
| `ll-format`  | the on-disk `.vss` file format (column chunks, index sections, CRC) |
| `ll-index`   | HNSW (vector) index + quantization + recall measurement |
| `ll-text`    | inverted index + BM25 |
| `ll-graph`   | edge (CSR) adjacency index |
| `ll-wal`     | write-ahead log |
| `ll-engine`  | memtable + MVCC + write loop + flush |
| `ll-catalog` | schema / tables / columns |
| `ll-cost`    | selectivity estimation + cost-based strategy selection |
| `ll-query`   | the `Source` abstraction, the hybrid executor, and the `Database` facade |
| `ll-bench`   | the benchmark harness |

If you're finding your way around, start at `crates/ll-query/src/exec.rs` (how a query is
planned and fused) and `crates/ll-query/src/database.rs` (the public API).

## Making a change

1. **Open an issue first** for anything non-trivial, so we can agree on the approach before
   you invest time.
2. Branch from `main`. Keep the change focused — one logical change per PR.
3. **Add or update tests.** Correctness is the project's whole value proposition; a behavior
   change without a test won't be merged. Match the style of the existing tests in
   `crates/*/tests/`.
4. Run `cargo test`, `cargo clippy --workspace --all-targets`, and `cargo fmt --all` before
   pushing — CI runs all three.
5. Write a clear PR description: what changed, why, and how you verified it. If it touches the
   on-disk format or a correctness guarantee (MVCC, recovery, recall), say so explicitly.

## Style

- Match the surrounding code: the codebase favors clear names, doc comments that explain
  *why* (not just *what*), and comments that call out the non-obvious invariant being upheld.
- Prefer correctness and clarity over cleverness. Where a fast path exists, keep an exact,
  obviously-correct fallback alongside it (as the planner does).

## Design docs

The canonical design lives in [`TechSpec/`](TechSpec/):

- `LL_Architectural_Specification.md` — full architecture.
- `LL_Decisions_Delta.md` — corrections that supersede the spec (authoritative when they
  conflict).
- `LL_FileFormat_ByteLayout.md` — implementation-grade on-disk format.

## License

By contributing, you agree that your contributions are licensed under the project's
[Apache-2.0](LICENSE) license.
