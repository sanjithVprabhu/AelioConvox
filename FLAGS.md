# FLAGS — implementation findings against `AELIO_DSL_MOTHER.md`

Findings, not failures. Format: `{id, section(s), what I found, options, recommendation}`. Provisional choices are marked `PROVISIONAL` in code comments. Blocking only if it gates correctness.

---

### F-001 — Workspace location (repo layout) — `§29`
**What:** The handoff says "use as repo `CLAUDE.md`" and names a fresh 8-crate Cargo workspace (`aelio-sol` … `aelio-cli`), but this monorepo already contains unrelated Rust (`Aelio DB/…`) and TS (`packages/`, `server/`). The mother doc does not pin a directory.
**Options:** (a) new top-level dir `aelio-os/` with its own workspace `Cargo.toml`; (b) place crates under repo-root `crates/`; (c) separate repo.
**Recommendation → (a) `aelio-os/`.** Clean separation from the legacy Aelio DB kernel, no workspace collision, easy to lift into its own repo later. `PROVISIONAL` — non-correctness; move is mechanical.

### F-002 — Store backend for P0 — `§24`, `§29`, Decision Log (Part VIII)
**What:** The doc commits Aelio DB as *the* store and says `aelio-store` "keeps a trait boundary for test doubles only." P0's §30 list needs Once/CAS and persistence, but P0's definition of done (login golden flow, replay bit-identity) is expressible against an in-memory double.
**Options:** (a) implement the `aelio-store` trait + in-memory double for all of P0, defer the Aelio DB binding to when P1 storage lands; (b) bind Aelio DB now.
**Recommendation → (a).** P0 exit criteria (§30) are all in-memory-satisfiable; the trait keeps Aelio DB a drop-in. `PROVISIONAL`.

### F-003 — `aelio-store` in the P0 dependency graph — `§29`
**What:** §29 lists `aelio-store` among the crates but the P0 build order (§30) touches storage only via Once/CAS and persistence. Dependency direction is "strictly downward"; the exact edges aren't drawn.
**Options:** (a) `aelio-kernel` depends on an `aelio-store` *trait* crate (store trait + in-memory double), inverting the concrete Aelio DB dep out of the kernel; (b) kernel owns persistence directly.
**Recommendation → (a) trait in `aelio-store`, kernel depends on the trait.** Matches "trait boundary for test doubles" and keeps the kernel Aelio DB-agnostic. `PROVISIONAL`.

### F-004 — Canonical float "shortest round-trip" formatting — `§4.3`
**What:** §4.3 mandates floats as "IEEE-754 f64, shortest round-trip form, always carrying a decimal point," `-0.0 → 0.0`, and cross-platform stability is a §27 property test. Rust's `{}`/`ryu` give shortest round-trip but not automatically "always a decimal point" (e.g. `2.0` prints `2`), and `-0.0` prints `-0`.
**Options:** (a) use `ryu` then post-process to guarantee a decimal point and normalize `-0.0`; (b) hand-roll a Grisu/Ryū-equivalent.
**Recommendation → (a).** `ryu` is the shortest-round-trip authority; a thin normalization layer enforces the decimal-point + `-0.0` rules. Covered by the int-vs-float and float-canonical conformance vectors. `PROVISIONAL` pending the cross-platform vector passing on CI.

### F-005 — Structural imprint over `var` resolution — `§4.1.3`
**What:** §4.1.3 says structural hashing resolves `var` values before hashing, and §4.1.4 says program-bearing bags (`fn`/`flow`) have *no* structural imprint. For `aelio-sol` in isolation there is no execution context to resolve a `var` path against.
**Options:** (a) `aelio-sol` computes structural imprints only over already-resolved, program-free bodies and returns a typed error (`ProgramBearing` / `UnresolvedVar`) otherwise, leaving `var` resolution to the kernel; (b) pull resolution into sol.
**Recommendation → (a).** Keeps `aelio-sol` zero-dep and pure; the kernel resolves `var` before asking sol to hash. `PROVISIONAL`.

### F-006 — Once across intentional Park — `§8.4`
**What:** Intent-without-result is fail-loud for crash recovery, but an intentional `Park` *inside* `Once` would leave status=`intent` until resume completes. Resume must not treat that as unknown-outcome.
**Options:** (a) park-aware Once status `parked`; (b) forbid Park inside Once at plan-time; (c) resume path skips re-claim (current) and only fails on *new* claim while intent open.
**Recommendation → (c) for P0.** Login golden has no Park inside Once. Plan-time forbid (b) is a clean P1 tightening. `PROVISIONAL`.

### F-007 — Annex location — handoff Phase 1
**What:** Handoff writes annexes to `docs/annexes/`; mother doc lives under `docs/claude_context/` with a root symlink `AELIO_DSL_MOTHER.md`.
**Recommendation → keep both.** Root symlink satisfies "repo root" references; annexes under `docs/annexes/`.

### F-008 — §9 compute catalog incomplete — `§9`, `F3`
**What:** `aelio-kernel/src/compute.rs` shipped only numeric/compare/logic/string/`is_type`/`blake3`, plus `pull`/`exists` in the Expr layer. The **locked §9 catalog** also mandates: structure `merge, drop, keep, path_copy`; list `count, append, first, last, slice, list_contains`; validate `matches_format`; and L0-C ledgered nondeterminism `now, uuid, random`. F3's completion check over-claimed ("≥3 vectors per op"). This is a P0 gap (handoff P0 step 4 = "Compute v0 §9/F3").
**Options:** (a) implement the pure ops in `compute::apply` now (merge/drop/keep/count/append/first/last/slice/list_contains), route `path_copy` as pure `(container, from, to)`; (b) leave list/structure to P1.
**Resolution → (a).** Pure ops added to `compute::apply` with vectors + unit tests. `path_copy` implemented as `(container, from, to)` (3-arg) — F3 said 2-arg "from,to" assuming an implicit bag edit; refined to explicit container for purity (compute stays pure, §A9.2). `matches_format` routed through the validator registry (§5.4) in the Expr layer alongside `exists`. `now/uuid/random` implemented as **L0-C ledgered nondeterminism** (§12.2 `nondet_value` INJECT entry on live, replayed from ledger per §12.3) — kept out of pure `compute::apply`.
**Decision Log:** 2026-07-27 · §9/F3 · path_copy signature refined 2-arg→3-arg (explicit container) · rationale: preserves compute purity (§A9.2); no locked invariant weakened.

### F-009 — §4.4 limits defined but not enforced — `§4.4`, `§4.2`
**What:** `aelio-sol`'s `Limits::check` (depth 32 / 1024 keys-per-map / 10k list len / 1 MiB canonical) was implemented and exported but **never called** by the kernel — bag writes could exceed the locked v0 caps unchecked. A locked invariant with no enforcement point.
**Resolution → enforce on every commit.** `Executor::write` and the `Const` whole-bag write now call `enforce_limits` → `Limits::default().check(bag)`, mapping any violation to `Budget.Size` (§11), so the bag invariant holds at all boundaries (persistence/Call/Park, §4.1). Tests in `limits.rs` (depth/keys/list caps reject; within-limits passes). No locked invariant weakened — this *adds* the missing enforcement. Deployer-tunable limits (§4.4 "tunable downward") left as future work; P0 uses defaults.

### F-010 — closed-schema not fully closed — `App E`, `§3`, `§4.4`
**What:** The parser rejected unknown **ops** but silently ignored unknown **fields** on a node, so a hallucinated/misspelled field (`arg` for `args`) parsed with the real field defaulting — violating "Unknown fields ⇒ plan-time reject" (App E) and the closed-schema threat-model guarantee (§3). Also, §8.2's "Const literal charged against §4.4 at plan time" was only enforced at runtime.
**Resolution → close both at plan time.** `instr::check_known_fields` validates every node's keys against a per-op allow-list (App E), rejecting extras as `Shape`. `plan::check_const_limits` charges every `Const` literal against §4.4 at compile, rejecting oversized literals as `Budget.Size`. Tests in `closed_schema.rs`. No locked invariant weakened — adds the missing rejections.

---
## Triage (2026-07-26)

| id | status |
|----|--------|
| F-001 workspace `aelio-os/` | **accepted** — built |
| F-002 store trait + memory for P0 | **accepted** — `aelio-store` |
| F-003 kernel→store trait | **accepted** |
| F-004 float canonical via ryu | **accepted** — sol tests green |
| F-005 structural imprint pre-resolved | **accepted** |
| F-006 Once/Park interaction | **deferred** P1 — login OK |
| F-007 annex/mother paths | **accepted** |

*This was the P0 triage snapshot. The later full-system findings below supersede its
“non-blocking” conclusion for an open-source production release.*

---

### F-011 — Two runtime implementations are not connected — `§22`, `§29`, `§31–§35`
**What:** The shipping TypeScript server under `server/` + `packages/core/` does not import, invoke,
or communicate with `aelio-os`. The Rust workspace now proves kernel semantics, but no production
request reaches it. Conversely, the existing TS turn pipeline predates the mother document and
cannot inherit its Planner, ledger, continuation, gate, or replay guarantees merely because the
Rust tests pass.

**Options:** (a) make the Rust runtime a local service and keep TypeScript as control plane/channel
adapters; (b) compile the kernel to a Node native/WASM module; (c) reimplement the locked semantics
in TypeScript.

**Recommendation → (a), BLOCKING open-source production claim.** A versioned local protocol keeps
the trusted kernel single-sourced, contains crashes, and lets the existing server/adapters migrate
incrementally. Native bindings have a smaller hop but a much larger build/distribution matrix;
duplicating the interpreter destroys the “one executor” replay guarantee.

**Status 2026-08-01 → PARTIALLY RESOLVED.** `aelio-server` is now the authoritative Rust process;
`aelio-runtime`, `aelio-agent-api`, and `aelio-db-api` are mounted together, and the host boundary is
versioned/authenticated. The remaining blocking half is Phase 6/8 of the unified master plan:
adaptive agent choices must become pinned runtime artifact invocations, and legacy TypeScript
decision modules must leave the production dependency graph.

### F-012 — `aelio-store` has no Aelio DB production implementation — `§24`, `§29`, F10
**What:** `aelio-store` contains the trait and `MemoryStore` only. The durable continuation/WAL
logic is tested across reconstructed instances using the shared memory double, but not across an OS
process or against Aelio DB. The mother document explicitly commits Aelio DB as the production store.

**Recommendation → implement a concrete Aelio DB store after F-011 fixes the process boundary, then
run kill/restart, CAS-conflict, partial-WAL, and tenant-isolation integration tests. BLOCKING.**

**Status 2026-08-01 → RESOLVED.** `aelio-store::EmbeddedStore` is the production implementation over
the embedded Aelio database; runtime startup uses it and keeps `MemoryStore` as the test double.
Restart/CAS/recovery tests exist. Broader artifact repositories and crash-window matrices are Phase
1/9 work, not a missing store binding.

### F-013 — Synchronous target closures cannot enforce hanging-call deadlines — `§8.4`, `§10.1`
**What:** registry declarations require `DeadlineCompliant`, and active elapsed time is checked after
return, but a closure that never returns cannot be interrupted. This does not yet satisfy “Timeout
reaches inside hanging external calls.”

**Recommendation → async adapter contract with cancellation/deadline propagation in the Rust
runtime service. BLOCKING for untrusted/live adapters.** Reverse SDK calls should use Appendix H’s
authoritative server deadline rather than an in-process closure.

### F-014 — Park inside Once remains an ambiguous crash state — `§8.4`
**What:** the current resume cursor skips a second claim in-process, but after restart the Once row
still says `intent`; continuation recovery can resume because it does not call `once_begin` again.
An operator or competing start using the same key sees unknown outcome. The state is safe
(fail-closed) but operationally indistinguishable from an actual crash during an effect.

**Recommendation → add a durable `parked` Once state tied to continuation hash, or reject Park under
Once in the Planner for v0. Prefer the Planner rejection until evidence requires the feature.**

### F-015 — App I handler frame representation refinement — `App I`
**What:** durable Try-handler frames store the pinned handler index, while App I’s prose shows the
handler code-prefix. Because the envelope pins the exact flow revision, the index is deterministic
and sufficient, but the byte format differs from the illustrated normative field.

**Recommendation → store both `handler_index` and `handler_prefix` and cross-check them at decode;
this preserves O(1) resume and makes the envelope self-explanatory.**

### F-017 — Flow Forge v0 (prompt → draft → plan → store) — `§15/§18/§31`, cold path
**What:** Product need: enter a natural-language prompt, draft a closed App E flow, Planner-validate, optionally store. Mother doc allows LLM as author under a gate; full §16 artifact gate + pathway prototypes are P1. No end-to-end “forge” unit existed (ProposePath emits ability paths, not kernel trees).
**Options:** (a) minimal forge over a vendor Call catalog (`forge.say@1`) with mock + OpenAI drafters, HTTP `/v1/flows/forge` + CLI `aelio forge`; (b) wait for full gate.
**Recommendation → (a) PROVISIONAL.** Catalog starts tiny; expand Call ids + promotion wiring next. Does not weaken locked ISA — Planner still rejects illegal trees.

---

### F-018 — Prism query language vs §10.4 single-op AST — `§10.4`
**What:** Product needs one multimodal recall language (`from/match/where/select/limit/into`, RRF fusion, mandatory select+limit, no joins). Engine already runs `HybridQuery`+RRF. Mother §10.4 / `QueryAst` is one op per query. Prism is the natural façade; single-op becomes sugar.
**Options:** (a) ship Prism envelope in `aelio-query` + lowerer in `aelio-db-query`, keep `QueryAst` for simple Calls, amend §10.4 via Decision Log so flexible reads are Prism; (b) keep two languages forever.
**Resolution 2026-08-01 → (a), RATIFIED.** Mother Amendment #4 makes the implemented closed
multimodal Prism envelope canonical; `QueryAst` is compile-time sugar. Parser, physical-schema,
projection, vector-dimension, graph-budget, adversarial, database API, and server embedding paths
are implemented and tested. Ordering/cursors/general scans remain deliberately absent in v1.

---

### F-019 — Mint prompt factory (root axiom + Aelio DB shelf) — `§10.3`, App J
**What:** Product needs a command-shaped factory that mints versioned system-prompt artifacts (body + slots + output contract + description) from a human **root** axiom, stores them in Aelio DB, and recalls them via Prism. Mother App J already defines the coin shape; `aelio-prompt` was compose-only.
**Options:** (a) Mint inside Aelio (`aelio-prompt` artifact + root lock + `aelio-runtime` MintShelf over Aelio DB + CLI `aelio mint` / HTTP `/v1/mint`); (b) keep free-text prompts in the TS harness.
**Recommendation → (a) PROVISIONAL.** Root (`aelio.mint.root@1`) is never mintable. Sufficiency refusal is required (mock refuses blood-type-from-demographics). OpenAI drafter optional. Forge wiring to pin Mint coins is next.

**Status 2026-08-01 → PARTIALLY RESOLVED.** `aelio-prompt::mint`, `MintShelf`, tenant-filtered Prism
recall, CLI/HTTP Mint, root immutability, mock/OpenAI drafters, and sufficiency refusal exist. Root
and minted prompts now lower into the schema-versioned unified artifact registry; proposed prompts
cannot be recalled. This flag remains open only for the prompt-class shadow/canary predicate and
Forge pinning of admitted Mint artifacts.

---
