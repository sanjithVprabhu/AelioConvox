# FLAGS — implementation findings against `AELIO_DSL_MOTHER.md`

Findings, not failures. Format: `{id, section(s), what I found, options, recommendation}`. Provisional choices are marked `PROVISIONAL` in code comments. Blocking only if it gates correctness.

---

### F-001 — Workspace location (repo layout) — `§29`
**What:** The handoff says "use as repo `CLAUDE.md`" and names a fresh 8-crate Cargo workspace (`aelio-sol` … `aelio-cli`), but this monorepo already contains unrelated Rust (`Sunjet/Astrolobe/…`) and TS (`packages/`, `server/`). The mother doc does not pin a directory.
**Options:** (a) new top-level dir `aelio-os/` with its own workspace `Cargo.toml`; (b) place crates under repo-root `crates/`; (c) separate repo.
**Recommendation → (a) `aelio-os/`.** Clean separation from the legacy Astrolobe kernel, no workspace collision, easy to lift into its own repo later. `PROVISIONAL` — non-correctness; move is mechanical.

### F-002 — Store backend for P0 — `§24`, `§29`, Decision Log (Part VIII)
**What:** The doc commits Sunjet as *the* store and says `aelio-store` "keeps a trait boundary for test doubles only." P0's §30 list needs Once/CAS and persistence, but P0's definition of done (login golden flow, replay bit-identity) is expressible against an in-memory double.
**Options:** (a) implement the `aelio-store` trait + in-memory double for all of P0, defer the Sunjet binding to when P1 storage lands; (b) bind Sunjet now.
**Recommendation → (a).** P0 exit criteria (§30) are all in-memory-satisfiable; the trait keeps Sunjet a drop-in. `PROVISIONAL`.

### F-003 — `aelio-store` in the P0 dependency graph — `§29`
**What:** §29 lists `aelio-store` among the crates but the P0 build order (§30) touches storage only via Once/CAS and persistence. Dependency direction is "strictly downward"; the exact edges aren't drawn.
**Options:** (a) `aelio-kernel` depends on an `aelio-store` *trait* crate (store trait + in-memory double), inverting the concrete Sunjet dep out of the kernel; (b) kernel owns persistence directly.
**Recommendation → (a) trait in `aelio-store`, kernel depends on the trait.** Matches "trait boundary for test doubles" and keeps the kernel Sunjet-agnostic. `PROVISIONAL`.

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

*Open items above are all `PROVISIONAL` and non-blocking. None weakens a locked invariant. Amendments (if any arise) will be proposed here first with a Decision Log entry, per handoff Authority rule #3.*
