# Aelio Production Spine Backlog

**Status:** derived from frozen glossaries (v0.1)
**Sources:** [`AELIO_L0_GLOSSARY.md`](./AELIO_L0_GLOSSARY.md), [`AELIO_L1_GLOSSARY.md`](./AELIO_L1_GLOSSARY.md), [`AELIO_AGENT_OS.md`](./AELIO_AGENT_OS.md)
**Rule:** every implementation task cites glossary ids. Design first; then code.

---

## Ranking method

Priority = **(blocks Agent OS loop)** × **(partial today)** × **(production risk)**.

| Tier | Meaning |
|---|---|
| **P0** | Without this, retrieve→act→persist→learn loop is unsafe or fake |
| **P1** | Needed for credible multi-tenant production |
| **P2** | Completeness / latency / open-source polish |
| **P3** | Aspirational dictionary surface |

---

## P0 — Close the OS loop

| # | Work item | Glossary ids | Why |
|---|---|---|---|
| 1 | Enforce **Budget `ms`** and real **Timeout** in sync/async eval | `L0.Budget`, `L0.Timeout` | Nested budgets are a kernel safety claim; stubs lie |
| 2 | Durable **Sense.Session** hydrate (open_loops, last_seen from store; not stubs) | `L1.Sense.Session`, `L1.Sense.Env` | Cockpit must reflect PCB truth after Park |
| 3 | End-to-end **Bind → Policy wrap → Once → Invoke.Call → Ledger → Sig match/extract** | `L1.Bind.*`, `L1.Policy.Evaluate`, `L0.Once`, `L1.Invoke.*`, `L1.Sig.*` | Only safe tool syscall path |
| 4 | **SigMismatch never guesses**; interpret + propose branch | `L1.Sig.Match`, `L1.Invoke.Interpret`, `L1.Invoke.Extract` | Silent wrong extractors corrupt state |
| 5 | **Resume re-checks Policy + State** after Park | `L0.Park`, `L1.Policy.Evaluate`, `L1.State.Read` | Stale auth is a security hole |
| 6 | **Learn promote gates** (evidence thresholds, effectful tenant gate) + demote on tool invalidate | `L1.Learn.Promote`, `L1.Learn.Demote`, `L1.Registry.Invalidate` | Install/uninstall asymmetry |
| 7 | Persist **flow_instances / states / turns** with compare-and-swap / version discipline | Agent OS §7, Aelio DB data model | Sense/Recall/Learn read lies if commits race |

---

## P1 — Production multi-tenant credibility

| # | Work item | Glossary ids | Why |
|---|---|---|---|
| 8 | Tenant isolation structural (per-tenant tables or mandatory tenant predicate + authz) | Filesystem / `L1.Sense.Tenant` | Shared WAL without isolation is not production |
| 9 | Repair **registry** wiring (`Bind.Normalize` → registered ids; demote phone/email from L0 core) | `L1.Bind.Normalize`, L0 §2.2 | Matches agnostic kernel contract |
| 10 | **Understand** production path: declared-intent first; shrink hardcoded depth lists; closed LLM JSON only | `L1.Understand.ClassifyDepth`, `ClassifyIntent`, `Extract` | Bootstrap heuristics must not become permanent thesaurus |
| 11 | **Recall** shapes L2 + `RewriteQuery`; stop sole HybridBroad-on-clause | `L1.Recall.*`, `L1.Understand.RewriteQuery` | Query construction incompleteness |
| 12 | **Judge.Confidence** calibration hooks per ability/tenant | `L1.Judge.Confidence` | S⇄L thresholds must be measurable |
| 13 | **Express** template-first for shallow; grounded Synthesize with claim ids | `L1.Express.Template`, `Synthesize`, `L1.Judge.Groundedness` | Cost + honesty |
| 14 | **Remember** open-loop write/read into Sense; no loops on greetings | `L1.Remember.OpenLoop`, `L1.Sense.Session` | Proactive without spam |
| 15 | Idempotency / Once keys durable across processes | `L0.Once`, `L1.Invoke.Call` | Multi-replica correctness |

---

## P2 — Completeness & open-source polish

| # | Work item | Glossary ids |
|---|---|---|
| 16 | L0-B expand: Decimal, rich pure-time, ValidateSchema | L0-B categories |
| 17 | L0-C `Schedule` / `Cancel` / `Emit` fully wired to jobs table | `L0.Schedule`, `Cancel`, `Emit` |
| 18 | `Learn.Explore` ε-greedy comparative runs | `L1.Learn.Explore` |
| 19 | `Observe.*` admin parity with `/admin/db` + proposals UI | `L1.Observe.*` |
| 20 | Proactive Decide table + Cadence + BudgetCheck | `L1.Proactive.*` |
| 21 | Graph recall + entity link writes | `L1.Recall.Graph`, `L1.Remember.LinkEntity` |
| 22 | Aspirational L0-A: `Par`, `Retry` (idempotent-only), `Debounce` | L0-A defer list |

---

## P3 — Dictionary completeness (do not block spine)

- Full L0-B numeric/string catalog parity with `aelio_dsl_dictionary.md`
- `Sense.Location`, `Understand.Tokenize` / `DetectLanguage`
- `Judge.Verify`, `Express.Style`, `Remember.Consolidate` / `Decay`
- Fancy recall shapes beyond HybridBroad / FilterThenRank

---

## Suggested implementation waves

```text
Wave A (P0 kernel truth)
  Timeout/Budget ms → durable Sense hydrate → Park resume policy re-check
  → Bind/Policy/Once/Invoke/Sig golden path → Promote/Demote gates

Wave B (P1 product truth)
  Tenant isolation → repair registry → Understand/Recall upgrade
  → Judge calibration → Remember loops → durable Once

Wave C (P2 platform)
  Schedule/Proactive/Observe → Explore → Graph → optional L0-A
```

Each wave ends with: glossary status flips (`partial` → `implemented`) + simulation/golden trace updates (`AELIO_RUST_12_SIMULATIONS` / golden_traces).

---

## Explicit non-work (until glossary changes)

- New domain L0 combinators
- LLM-as-kernel turn controller
- Rewriting L0 closed set to expand branching factor for Compose
- Personality in situation keys

---

## Tracking

When a P0/P1 item lands, update:

1. Status column in L0/L1 glossaries
2. This backlog (strike or move to Done)
3. Decision log if semantics changed

**Done definition for “production spine”:** P0 complete + P1 items 8–15 at `implemented` or explicitly waived with risk note.
