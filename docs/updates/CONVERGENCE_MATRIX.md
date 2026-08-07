> **Reference-design notice (2026-08-07).** This matrix reviews `HARNESS_KERNEL_V4.md`, an
> **external reference design**, not this repository's actual system (the real spec is
> `AELIO_DSL_MOTHER.md`, implemented under `aelio-os/`). Its 20 components (op catalog, effect
> driver, skeleton extraction, matching pipeline, etc.) do not correspond 1:1 to real `aelio-os`
> modules. See `FLAGS.md` entry **F-032** for the terminology map and which findings were
> actually checked against real code.

# Convergence Matrix

**8 generators × 20 components.** Every cell is `✓` (closed, with a check), `○` (open, needs work), `—` (non-applicable, with a reason), or `!` (accepted risk, named owner).

**Stopping rule:** matrix fully populated; G-VER and G-AUTH structurally closed; two consecutive review passes producing zero findings in `✓` cells; all six adversarial stances run.

Findings in `○` cells do not reset the counter — they mean the matrix isn't done. Findings in `✓` cells mean the check was wrong, and reset it.

---

## Generators

| ID | Question asked of every component |
|---|---|
| **DET** | Can this introduce a value differing between two executions with identical inputs? Trace it to `output_hash`. |
| **BND** | Does any byte here cross the plane boundary? Is it on the allow-list? |
| **DUP** | Can this path invoke a tool effect? Under what circumstances can it run twice? |
| **PRT** | What happens on empty, null, zero, one, max, duplicate, type boundary? |
| **VER** | What inputs can change without the code changing? What does a change invalidate? |
| **AUT** | Does a security decision depend on this? Does it trace to a content hash or to mutable state? |
| **VOL** | What happens at once a month? Once a second? |
| **SCL** | What happens with three replicas, two concurrent turns, failover mid-operation? |

---

## Matrix

| # | Component | DET | BND | DUP | PRT | VER | AUT | VOL | SCL |
|---|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| 1 | Op catalog | ✓ | — | — | ✓ | ✓ | — | — | — |
| 2 | Evaluator / value repr | ✓ | — | — | ✓ | ✓ | ○ | — | ✓ |
| 3 | Effect driver | ✓ | ✓ | ✓ | ✓ | — | ✓ | ○ | ✓ |
| 4 | Tool boundary | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ○ | ○ |
| 5 | Budget accounting | ✓ | — | — | ○ | — | — | ✓ | ○ |
| 6 | Trace / journal | ✓ | ✓ | ✓ | ○ | ✓ | — | ○ | ✓ |
| 7 | Ledger | ✓ | ✓ | — | ○ | ✓ | ✓ | — | ✓ |
| 8 | Skeleton extraction | ✓ | ✓ | ✓ | ○ | ✓ | — | — | — |
| 9 | Signature / content addressing | ✓ | ✓ | — | — | ✓ | ✓ | — | — |
| 10 | Matching pipeline | ✓ | ✓ | — | ○ | ✓ | ✓ | ✓ | ○ |
| 11 | Conductor | ✓ | ✓ | ✓ | ○ | ✓ | ✓ | — | — |
| 12 | Authoring pipeline | ✓ | ✓ | ✓ | ○ | ✓ | ✓ | ✓ | ○ |
| 13 | Promotion machinery | — | ✓ | ✓ | ○ | ✓ | ✓ | ✓ | ✓ |
| 14 | Schema graph / path binding | ✓ | ✓ | — | ✓ | ✓ | ✓ | — | ○ |
| 15 | Control-plane storage | ○ | ✓ | — | ○ | ✓ | ✓ | ○ | ○ |
| 16 | Data-plane storage | ✓ | ✓ | — | ○ | ✓ | — | ○ | ✓ |
| 17 | Plane transport | ○ | ✓ | ✓ | ○ | ✓ | ✓ | — | ○ |
| 18 | SDK registration | — | ✓ | — | ✓ | ✓ | ✓ | — | ✓ |
| 19 | Observability / audit | ✓ | ✓ | ✓ | ○ | ✓ | ○ | ✓ | ✓ |
| 20 | Renderers | ✓ | ✓ | — | ○ | ✓ | — | — | — |

**Totals:** 160 cells — 96 `✓`, 33 `○`, 31 `—`, 0 `!`.

---

## Closed cells and their checks

| Cell | Closure |
|---|---|
| DET × 1, 2 | §2.1–2.10; cross-arch CI test; clippy `disallowed_types` on `HashMap`; `preserve_order` deny; fast-math scan |
| DET × 4 | §5.2 canonical ordering, cursor pagination, continuity verification, Shadow determinism probe |
| DET × 6, 7, 9 | §2.9 canonical serialiser; structured error enums; BLAKE3 over sorted keys |
| DET × 8 | §6.3 float/decimal never reordered; pairwise summation with fixed order |
| DET × 10 | §7.2 cache key includes `validity_bucket`, `tenant_tz`, `schema_version`, `join_path_set` |
| DET × 20 | §8.6 single shared renderer, property test for structural coverage |
| BND × all | §11.4 transmission allow-list with fail-on-unlisted test; §12.2 skeleton-only storage; §5.9 taint on `emit`/`fail`; §7.9 residue stores kinds not values; §11.1 data-plane authoring |
| DUP × 3, 4 | §5.3 shadow stubbing; §5.4 keys incl. `element_index`; §5.5 all-or-nothing; no write retry |
| DUP × 8, 13 | §6.5 golden cases read-only by construction |
| DUP × 11, 12 | §8.1 `Reply` has no effect-driver access; Dry mode over sample data |
| DUP × 19 | §14.3 audit never re-executes writes; structural comparison only |
| PRT × 1 | §2.2–2.5 return types, null semantics, count split, empty-vs-error |
| PRT × 4, 14, 18 | §5.1 completeness classes; §13.2 mandatory cardinality; §13.6 non-aggregatable types |
| VER × all | §17.1 `SystemVersion` exhaustive destructuring; §17.3 op `impl_hash` Merkle root; §17.4 migration procedure |
| AUT × 3, 9 | §5.6 content-addressed effect set; §17.2 `Verified<T>` |
| AUT × 4, 14 | §5.7 principal-scoped; grants on every path entity |
| AUT × 7, 13, 18 | §12.7 ledger `system_version`; §9.4 CAS transitions; §11.5 required `row_scope` |
| VOL × 5, 12, 13 | §9.3 dual promotion routes; §3.5 separate audit budget |
| VOL × 10, 19 | §14.2 cold-start separation; §14.3 sampling weights |
| SCL × 2, 6, 7 | §10.2 session serialisation; §10.4 ledger segments; replica-local traces |
| SCL × 13, 16, 19 | §9.4 CAS; §14.1 raw counters not ratios; single-instance guard |
| SCL × 3, 18 | §3.7 bounded blocking pool; idempotent registration |

---

## Open cells — the remaining work

Ordered by expected severity. This is the actual backlog.

### High

**DET × 15 (control-plane storage).** Astrolobe's own determinism is unexamined. Does vector search return identical top-k for identical input under concurrent writes? Does graph traversal return edges in a stable order? Neither affects `output_hash` directly, but unstable top-k makes Phase 1 nondeterministic, which makes match-rate metrics noisy and makes a false hit irreproducible when you try to investigate it. *Resolvable by build.*

**DET × 17 (plane transport).** Message framing, compression, and retry could reorder or duplicate control-plane messages. Plan delivery must be idempotent and order-independent. *Resolvable by review — do this one.*

**PRT × 5 (budget accounting).** What happens at exactly the limit? Off-by-one on `result_rows` at precisely 1,000,000. Does a budget of zero mean unlimited or immediately-exhausted? Classic boundary territory and cheap to close with a generated test.

**PRT × 6, 7 (trace, ledger).** Zero-step trace, zero-effect journal, a session with one turn, a segment with one entry. Chain verification over a single-entry segment is exactly where an off-by-one lives.

**SCL × 4 (tool boundary).** Concurrent `map_tool` from two turns against the same tool. Does the customer's tool tolerate the concurrency the kernel generates? Bounded pool helps but the bound is per-execution, not global. Needs a global in-flight cap per tool.

**AUT × 2 (evaluator).** Can a crafted value escape the taint bit? Deep-copy through a collection, dict key promotion, string interning. Needs an explicit audit of the value repr.

### Medium

**VOL × 3, 4 (effect driver, tool boundary).** Behaviour at one request per second sustained. Connection pooling, tool rate limits, backpressure into the budget. Currently unspecified.

**SCL × 10, 12 (matching, authoring).** Two replicas authoring the same novel intent simultaneously produce two `Draft` skeletons that are probably equivalent. §6.5 merges them at promotion, but until then the library has duplicates and stats fragment.

**PRT × 8, 10, 11, 12 (extraction, matching, conductor, authoring).** Empty constraint set, single-constraint intent, an intent with 50 constraints, a skeleton with zero holes, a skeleton with 30. Generated boundary matrix.

**SCL × 14, 15, 17.** Schema graph updated while a path resolution is in flight (partially addressed by §10.5 pinning, but the *control-plane* read path isn't pinned); Astrolobe under concurrent tenant writes; transport reconnect storms.

**AUT × 19 (observability).** Audit executions run with which principal? If they use an elevated principal to compare against the authoring path, that is a privilege boundary crossed for measurement purposes.

**VOL × 15, 16 (storage).** Retention, compaction, and index rebuild behaviour at scale. Also what a tenant with a single workflow and 10M executions does to HNSW recall.

### Low

**PRT × 13, 16, 19, 20.** Promotion with zero golden cases (should be rejected — verify), empty `redb` on cold boot, metrics with zero denominator, renderer given a zero-node AST.

**DET × 3 is `✓` but adjacent:** journal replay with a zero-length journal.

---

## Adversarial stances

| # | Stance | Run? | Produced |
|---|---|:-:|---|
| 1 | Silent-wrong-answer adversary | ✓ | residue check, fan-out, pagination sampling, ambiguity |
| 2 | Malicious end user | ✓ | authoring tiers, taint, join-path grants, schema-graph inference |
| 3 | Compliance auditor | partial | ledger, segments, `join_path_hash` — not run against the consolidated doc |
| 4 | **Operator at 3am** | **✗** | — |
| 5 | Customer's security reviewer | ✓ | all four boundary leaks |
| 6 | **Cost accountant** | **✗** | — |

Stances 4 and 6 are the highest-yield remaining work. Expect findings around error message quality, degradation runbooks, what a paged engineer can actually see, per-tenant spend caps, cost attribution for authoring versus matching, and what happens when a tenant hits a cap mid-turn.

---

## Sequence

1. Close **DET × 17** and **PRT × 5, 6, 7** — pure review, half a day, high value.
2. Run **stance 4 (operator)** and **stance 6 (cost accountant)** against v4. One day. Expect 8–15 findings.
3. Close **AUT × 2** (taint escape audit) — needs the value repr to exist, so pairs with M1.
4. Everything tagged *resolvable by build* waits for M1/M2. Do not review it further; reviewing produces speculation with the texture of findings, which consumes the same attention and yields nothing checkable.
5. Two clean passes against `✓` cells only.

**Predicted remaining findings before convergence: 15–25**, concentrated in stances 4 and 6 and in the `○` column. Then it should stop, because there is nowhere left for a finding of a known class to hide.
