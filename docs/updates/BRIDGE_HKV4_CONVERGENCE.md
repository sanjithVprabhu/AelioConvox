# Bridge → HKv4 Convergence Map

**Authority:** [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md) Task 9 · [`FLAGS.md` F-033](../../FLAGS.md)  
**Purpose:** One row per shared concept — prevents the orchestration bridge and HKv4 target from drifting into two permanent reuse systems.

Phase 1 fixes (#1 promotion σ, #6 effectful detection) already prove load-bearing semantics are shared. This map states how each bridge artifact **converges or retires**.

| Bridge concept | HKv4 concept | Convergence action | Retirement condition |
|---|---|---|---|
| `SituationKey` (intent class, slots, caps, recency bucket) | Phase 0 normalised structured intent → cache key | **Re-implement on harness-core** — add `validity_bucket`, `tenant_tz`, `schema_version` to key material per §7.2 | HKv4 intent normaliser owns key; bridge key deprecated when Starlark harness cache hits ≥95% of warm lookups |
| `TaskGraph` (heuristic / pinned DAG) | Harness AST (Starlark → parsed AST) | **Replacement** — bridge graphs are authoring evidence only; promoted artifact becomes AST + skeleton signature, not literal `AbilityPath` steps | No new TaskGraph pins after Starlark codegen green; existing pins migrate or expire |
| TaskGraph promotion / `observe_and_promote` | Skeleton signature + volume/evidence promotion ladder | **Re-implement on harness-core** — promotion gate uses `SystemVersion`, distinct-input thresholds, effectful path approval | Single promotion ladder in `aelio-convert` gate; orchestration promotion becomes thin wrapper calling same gate |
| `path_is_effectful` | Transitive effect set (§5.6) + `Verified<T>` | **Converges by re-implementation** — bridge uses registry `effectful` flag today; HKv4 requires content-addressed effect set + compile-time `Verified<T>` at authorisation | `path_is_effectful` deleted when `Verified<EffectSet>` wraps all execute paths |
| Orchestration ledger (`wavefront::LedgerEntry`, `args_hash`) | Trace + journal + ledger entry (step hashes) | **Re-implement on harness-core `exec`** — port per-step `inputs_hash`/`output_hash`; bridge ledger is test double until journal unifies | One journal format across kernel + agent; bridge ledger deleted |
| `std.*` tools (in-process catalog) | Op catalog entries (`impl_hash` Merkle root) | **Per-tool decision** — pure compute ops (`std.math.*`, `std.data.*`) → catalog entries; client-effect tools stay contract-bound, never catalog | Catalog Merkle root in `SystemVersion`; std tools that duplicate catalog ops removed |
| `canonical_hash` / `value_to_sol` (Phase 1) | `harness_core::canonical` (tagged wire, §2.9) | **Reconcile immediately (Task 2)** — one serialiser; `aelio-sol` re-exports or delegates; TaskGraph writer deleted or built on shared primitive | Second writer in `task_graph.rs` removed after port |
| Residue check (`abilities/residue.rs`) | §7.4 constraint decomposition + residue | **Converges by extension** — bridge heuristic stays for ProposePath; HKv4 residue runs on parsed AST bindings | Bridge residue deleted when skeleton binding carries full constraint set |
| `GraphSuspension` / NeedUser | Mid-harness park (continuation) | **Wire or delete (Task 3)** — either persist suspension for resume or remove dead types | Resume from `GraphSuspension` or types removed |
| Warm path (`LookupTier` Tier0/Tier1) | Skeleton cache hit | **Stays bridge-only until Task 10 proves reuse** — counters measure executions-per-key; if ~1:1, premise wrong | Replaced by content-addressed skeleton store when HKv4 matching pipeline owns lookup |

## Red flags (must not stay indefinitely)

- **Two canonical serialisers** — finding-grade; Task 2 blocks all other hashing work.
- **Two promotion ladders** — bridge `observe_and_promote` vs convert gate must share one reach-tiered gate.
- **Text-hashed harnesses** — forbidden by F-034; AST canonical re-render only.
- **Fake eval gate** — semantic stub returning `pass: true` (Task 3).

## References

- Phase 1 promotion-key fix: [`orchestrate.rs`](../../aelio-os/crates/aelio-agent/src/blocks/orchestrate.rs)
- Effectful fix: [`promotion.rs`](../../aelio-os/crates/aelio-agent/src/orchestration/promotion.rs)
- Work order: [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md)
