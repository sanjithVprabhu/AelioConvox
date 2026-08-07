# Phase 2 Instructions — for the Cursor agent

**Input:** `IMPLEMENTATION_VERIFICATION_REPORT.md` (Phase 1 report)
**Authority:** HKv4 (`HARNESS_KERNEL_V4.md`) per F-033
**This document is the work order for Phase 2.** Items are ordered; do not reorder without a FLAGS entry.

---

## 0. The product, restated — so the bridge does not become the product

Before any task: the objective is **reuse under proof**. A user's request either *matches* a previously-discovered abstract workflow (skeleton + new bindings, executed deterministically, authorised pre-execution) or is *authored* once and saved for every future request of the same shape. The five invariants — determinism, pre-execution authorisation, precision over recall, auditability, content-addressed trust — are the product. Starlark, the numeric tower, and taint are how those invariants are enforced, not features in themselves.

The Phase 1 report shows a working **orchestration bridge** (SituationKey → TaskGraph → promotion → warm path). That bridge is Aelio's existing reuse loop and it is fine as scaffolding. The risk in Phase 2 is that the bridge quietly becomes permanent — two reuse systems, two promotion ladders, two hashing schemes, drifting. Task 9 below exists specifically to prevent that. Every Phase 2 task should be checked against one question: *does this move the bridge toward the HKv4 loop, or entrench it?*

---

## 1. DECISION REQUIRED — Starlark text vs AST-as-JSON (resolve before writing codegen)

The report ratifies "LLM-generated Starlark" over HKv4 §8.3's AST-as-JSON, with the rationale "models write imperative code more reliably than they fill rigid AST holes."

**That rationale is half right, and adopting it as stated reintroduces the exact failure class §8.3 was designed to eliminate.** The empirical record: code-emission agents show ~2.4% parse-failure rates, and parse failure drops task success by >20 points. AST-as-JSON was chosen because grammar-constrained decoding of Starlark text is a local-inference feature, unavailable on hosted APIs — but structured-output JSON *is* available everywhere, and it makes syntax errors unrepresentable.

However: the "rigid AST holes" objection has real force if your AST schema is deep and the model fights it. So resolve it as a **hybrid, with one non-negotiable rule**:

- The model MAY emit Starlark **text** (its comfort zone).
- The text is immediately **parsed to an AST**, and from that point the AST is the ONLY authoritative artifact:
  - `harness_hash` = BLAKE3 over the **canonically re-rendered** source (parse → normalise → render), never over the raw emitted text. Two texts differing in whitespace, comments, or paren style must produce one hash.
  - Skeleton extraction (§6.3) runs on the parsed AST.
  - The verification rendering shown to Phase 3 and to the user is rendered from the AST by the same renderer.
- Parse failure enters the **capped repair loop (max 3)** with structured diagnostics, then escalates to Clarify. Never retry unbounded.
- Renderer round-trip property test: `render(parse(text))` re-parses to an identical AST, for every authored harness. This is what keeps text emission from smuggling non-canonical forms into the content-addressed store.

Record the resolution as a FLAGS entry (F-034) either way. If you keep pure text with text-hashing, say so explicitly and accept that identical programs will fragment the cache — but do not do this silently.

---

## 2. Port `harness-core` — DO NOT reimplement the stubs [first coding task]

The report scaffolded `harness-core` as stubs. **The full implementation already exists** — 2,111 lines, 54 passing tests — delivered alongside the checklist:

```
harness-core/src/numeric.rs      327 lines   Int/Float/Decimal, banker's rounding, NaN guard
harness-core/src/canonical.rs    221 lines   canonical serialiser, tagged wire format
harness-core/src/agg.rs          357 lines   null-policy aggregates, count split, pairwise sum
harness-core/src/taint.rs        381 lines   value taint, map-key absorption, PC-taint
harness-core/src/exec.rs         446 lines   CAS budget pool, journal, replay verification
harness-core/src/versioning.rs   367 lines   SystemVersion, Verified<T>, impl_hash Merkle
harness-core/tests/determinism.rs            pinned reference hashes
```

Port these files into `aelio-os/crates/harness-core/` over the stubs. Then:

1. **Re-pin the reference hashes.** The pinned values in `tests/determinism.rs` were computed with `blake3 = 1.5.0` on x86_64. Run the suite, take the printed hashes, pin them, and record the blake3 version in the lockfile note. From that point, a change to either hash is a `SystemVersion.serialiser` bump, never a fixture edit.
2. **Reconcile with `aelio-sol`'s canonical writer.** Phase 1 fix #2 built a §4.3-style writer inside `aelio-sol` (`canonical_bytes`, ryu floats, sorted nodes). There must be **one** canonical serialiser in the workspace. Either `aelio-sol` re-exports `harness_core::canonical`, or its writer is rewritten on top of it. Two independent canonical writers is a determinism split waiting to happen — this is finding-grade if left.
3. Wire `harness-core`'s clippy.toml scope to the whole workspace hashing/effect paths, not just `aelio-sol` (see Task 4).
4. All 54 tests green in the workspace, `cargo test -p harness-core` and `--release` producing identical hash output.

**Exit:** 54+ tests green; one canonical serialiser; hashes pinned with provenance.

---

## 3. Kill the fake gates [SEC-adjacent — before anything else ships]

Report Q3 admits: **eval closure still returns `pass: true`**, and `GraphSuspension` types exist unused.

A stub that returns success on an evaluation path is worse than no gate: everything downstream believes verification happened. This is the same class as "shadow passed" implying correctness that was never checked (§9.5).

1. Inventory every stub that returns `Ok`/`true`/`pass` on a gating path: eval closure, any promotion predicate, any policy check. `grep -rn "pass: true\|Ok(())" --include="*.rs"` and triage each hit.
2. Each one becomes **fail-closed** (`unimplemented_gate!` that refuses and logs) or is implemented now. A gate that cannot yet evaluate must refuse, not approve.
3. Either wire `GraphSuspension` into the NeedUser path (fix #4 built suspension — presumably these types were for it) or delete them. Dead types adjacent to a live feature confuse the next reader about what is enforced.
4. FLAGS entry listing every gate that is currently fail-closed-pending-implementation, so nothing silently ships approving.

**Exit:** zero gating paths return unconditional success; grep in CI to keep it that way.

---

## 4. Close the Phase 1 PARTIALs in determinism [KILL tier]

These four are marked PARTIAL in the report and each is a live I1 hole:

**4a. HashMap in wavefront runtime state (Q13).** Audit every `HashMap` in `wavefront.rs`: does its iteration order ever reach (i) a hash input, (ii) effect dispatch order, (iii) journal write order, (iv) anything serialised? If any — convert to `BTreeMap`. If provably none — annotate with the clippy allow and a one-line justification. "Runtime state" is exactly where this bug hides, because wavefront scheduling order can determine journal order.

**4b. Per-step hash comparison in replay (Q21).** Final `bag_hash` comparison detects divergence but localises nothing — a diverged replay currently tells you *that* something is nondeterministic, not *where*. Port `exec::verify_replay` + `Step` from harness-core: every step records `inputs_hash`/`output_hash`, and divergence reports the first diverging `seq`. Without this, the first real replay failure costs days instead of minutes.

**4c. Full-workspace parity CI (Q20, Q100).** Extend the aarch64 job from `aelio-sol`-only to the workspace, and add the debug-vs-release hash comparison: run every hash-producing test in both profiles, diff the printed hashes. Add the fast-math grep and the `preserve_order` feature-tree check from harness-core's workflow if not already present.

**4d. Journal ordering ahead of parallel Map (Q24).** Map is sequential today, so index-ordering is trivially true — which means **now** is when to encode the invariant, before parallelism makes it hard. Write the test that induces out-of-order completion (spawn with reversed delays) and asserts journal entries land in input-index order. It passes trivially today and becomes the regression net when Map goes parallel.

**Exit:** all four moved to PASS with named tests.

---

## 5. Effect boundary — the three contract-shape decisions that get baked in early

Section 7 is PARTIAL. Full effect driver work can wait for Starlark, but three decisions harden as soon as more tools register, so decide them now:

1. **Contract registration refuses without `effect_class`, `completeness`, `returns_entity`** (Q69). Every tool already registered gets backfilled; every new registration hard-fails without them. Retrofitting completeness onto fifty registered tools later is a migration; requiring it on the next one is a line of code.
2. **Shadow/dry write stubbing** (Q72–73): if any shadow-like or replay-adjacent execution mode exists in the bridge (promotion evidence runs?), verify writes cannot dispatch there. If promotion evidence collection ever executes an effectful path live, that is v2-L1 resurrected — check `promotion.rs`'s evidence flow specifically, since fix #6 just touched effectful detection.
3. **Idempotency key shape** (Q74–75): keys must be `(turn_key, effect_seq, element_index)` with `turn_key` client-supplied and retry-stable. The report says idempotency tests exist — verify the key *composition* matches, especially `element_index`, before more ledger entries accumulate under a wrong scheme.

**Exit:** registration refuses incomplete contracts; a written answer (with file refs) for 2 and 3.

---

## 6. `harness-core` Phase 2 completion — what the stubs were for

With the port done (Task 2), the remaining genuinely-new work before Starlark:

1. **Op catalog assembly** (§3.3): register the ported aggregate/numeric ops into a catalog with per-op `impl_hash`, Merkle root as `op_catalog` in `SystemVersion`. The report's `std.*` tools map into this catalog or stay bridge-only — decide per tool, record it.
2. **Trybuild compile-fail tests** for `Verified<T>` and `SystemVersion` (Q61, Q65). These are the actual structural closure; the runtime tests are secondary. `tests/ui/unverified_effect_set.rs` must fail to compile.
3. **Time module** (§2.6–2.7): UTC epoch ms, pinned tzdb shipped in-crate, `TemporalBinding` with tenant-tz validity buckets. Small, and it must exist before any caching of time-relative plans — check whether the bridge's warm path already caches anything time-relative; if yes, this is urgent, because a cached `LastNDays` plan is serving stale windows right now.
4. **Entity-key canonical ordering at the tool boundary** (§5.2 / Q78): every tool result sorted by declared entity key before entering the value graph. This can land pre-Starlark since the bridge's tool calls flow through the same boundary.

**Exit:** catalog with Merkle root; compile-fail tests failing correctly; time module with the tenant-midnight test; tool results canonically ordered.

---

## 7. Starlark embedding — only after 2, 3, 4, 6

The report's ordering instinct is correct: Section 8 is [KILL if wired] without substrate. Sequence within the embedding:

1. Resolve the Rust edition/toolchain requirement for `starlark` (needs edition2024-capable toolchain).
2. **Dialect lockdown asserted at startup** — config hash compared against `SystemVersion.dialect`; refuse to start on mismatch (Q68, Q81).
3. **Value type is the tainted wrapper from day one** (Q82). Do not wire raw values and bolt taint on later — bolted-on taint has seams, and the map-key laundering hole (N13) is exactly the kind of seam it produces.
4. **Budget threaded through the evaluator** — steps deplete the shared CAS pool; test a program that allocates past the heap limit to learn whether starlark's profiler enforces or approximates (Q84). The answer is a finding either way.
5. **Effect bridge**: `spawn_blocking` + mpsc/oneshot exactly as §3.7. No continuation suspension, no nested runtime.
6. **Runtime cycle check**: call stack of content hashes; A→B→A rejected at runtime, test included (Q85).
7. Then the codegen path per the Task 1 resolution: emit → parse → normalise → hash → static checks → execute → cache under `SystemVersion`.

**Exit:** a hand-written harness executes under locked dialect with taint, budget, and journal; replay is byte-identical; A→B→A rejected.

---

## 8. Residue check — verify the PASS is real [the single most important behaviour]

The report claims Q97 PASS on the Bangalore case. One passing case is necessary, not sufficient — the residue check is the load-bearing correctness gate and it needs the adversarial set (§14.5):

Run and record each: **extra constraint** (Bangalore — claimed passing), **dropped constraint** (request omits a filter the skeleton requires — must also reject), **aggregation swap** (mean vs median — same signature, must be caught at verification, not residue; confirm which gate catches it and that one does), **comparison boundary** (`>` vs `>=`), **field swap** (height vs weight), **source swap** (people vs employees), **null-policy variant** (must be a *different skeleton*, not a binding).

Also answer: in the current bridge, what plays the role of the constraint decomposition? If SituationKey matching can bind a request while silently dropping a clause qualifier, the bridge has the pre-residue bug HKv4 §7.4 exists to prevent — and the bridge is what's serving traffic today.

**Exit:** a residue test file with all seven cases, each asserting the rejecting (or catching) gate by name.

---

## 9. Bridge → HKv4 convergence map [prevents two permanent systems]

Produce a short mapping doc (this is thinking work, not code):

| Bridge concept | HKv4 concept | Convergence action |
|---|---|---|
| SituationKey | normalised structured intent → Phase 0 key | ? |
| TaskGraph | harness AST | ? |
| TaskGraph promotion / warm path | skeleton signature + promotion ladder | ? |
| `path_is_effectful` | transitive effect set (§5.6) + `Verified<T>` | ? |
| Orchestration ledger | trace + journal + ledger entry | ? |
| `std.*` tools | op catalog entries vs contract-bound tools | ? |

For each row: converges by replacement, converges by re-implementation on harness-core, or stays bridge-only with a retirement condition. Any row marked "stays indefinitely" is a red flag to surface, not bury. The promotion-key fix (#1) and the effectful fix (#6) both show the bridge and the target already sharing load-bearing semantics — un-mapped, they will drift back apart.

**Exit:** the table filled, as a FLAGS-referenced doc.

---

## 10. The kill-criterion instrumentation — do not defer this again

Nothing in the Phase 1 report measures **reuse**. The bridge has a warm path — which means the data exists *right now*: warm-path hits vs cold-path authorings, per SituationKey, per day.

Add counters (raw counters, never pre-computed ratios — §14.1) for: cold-path executions, warm-path hits, distinct SituationKeys, executions per key. Log daily. This number is the thesis: if executions-per-key sits near 1:1 once traffic is real, the architecture is correct and the premise is wrong, and it is far cheaper to learn that from the bridge's telemetry than after the full Starlark migration.

This is a day of work and it is the highest-information task in this entire document.

**Exit:** counters live; one week of data reviewed with executions-per-key distribution.

---

## Corrections to the Phase 1 report itself

Two scorecard entries are miscategorised — fix them so the scorecard stays trustworthy:

1. **Q103/Q104 marked FIXED via F-033** is a category error. F-033 documents the *framework authority shift* — a good FLAGS entry, but Q103 asks for the list of **unilateral micro-decisions** made where the spec was ambiguous (a division scale here, an error variant there), and Q104 asks **what incorrect implementation would still pass the current test suite**. Neither is answered by a migration record. Answer both properly: Q103 as a running list appended per session; Q104 as an honest gap statement per module (its answer is the next test file's table of contents).
2. **Q22 marked PASS** — `ReplayBackend::pop` refusing past-ledger reads is the right *behaviour*, but confirm the refusal is a **typed `JournalUnderrun` divergence** surfaced to the replay verifier, not a generic error swallowed by a caller. The distinction matters when the nightly job exists: an underrun must page, a generic error might not.

---

## Sequencing summary

```
Task 1 (decision)  ─┐
Task 2 (port)      ─┼─ this week, in order
Task 3 (fake gates)─┘
Task 4 (KILL partials) ── immediately after 2
Task 10 (reuse counters) ── in parallel, any day, do not let it slip again
Task 5, 6 ── substrate completion
Task 8 ── alongside 6 (residue tests need no Starlark)
Task 7 ── Starlark, last
Task 9 ── thinking work, before Task 7 starts
```

**Definition of Phase 2 done:** harness-core fully implemented and reconciled (one canonical serialiser); zero fake gates; all four determinism PARTIALs at PASS; residue adversarial set green; F-034 recorded; bridge convergence map written; reuse counters producing daily numbers. Then — and only then — Starlark.
