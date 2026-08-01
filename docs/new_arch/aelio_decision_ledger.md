# Aelio / Convox — Architecture Decision Ledger

Status: locked as of this session. Input to the full architecture document.

Companion artifacts:
- `aelio_dsl_dictionary.md` — complete instruction set, all rungs
- `HARNESS_AELIO_DB_DATA_MODEL.md` — storage spine, 26 current tables, target model

---

## 0. Product definition

**What it is.** An MCP-shaped layer pointed the other way. MCP standardizes tool exposure *to* a model the user already has. Aelio standardizes conversational exposure *of* a SaaS product to that product's own end users.

**Motion.** SaaS company installs the SDK on their backend, registers tools. Their customers get a conversational interface over the whole product surface — without the SaaS company building an agent, hiring an AI team, or exposing API keys.

**Shape.** B2B2C. The SaaS company pays; their customers talk. Multi-tenancy is a first-class structural concern, not a deployment detail.

**Objective.** A system that handles arbitrary API / database / LLM interaction, vertical-agnostic, with no pre-coded per-vertical logic.

**The core bet.** The LLM is not the router. Retrieval and selection are deterministic; the model synthesizes, speaks, and — at cold start only — proposes paths over a typed ability set. This is what makes the system defensible to a customer with a security review.

**Formal framing.** The LLM is a proposal heuristic over a typed ability set; the type system is the verifier. Type-directed program synthesis with a model as search heuristic instead of exhaustive enumeration. "Does this output go to synthesis or into another ability?" is answered by type unification, not judgment.

---

## 1. Locked decisions — product & scope

| # | Decision | Rationale |
|---|---|---|
| P1 | Vertical-agnostic; no pre-coded verticals | stated primary objective |
| P2 | Everything vertical-specific must be **declared**, never inferred | if inferred, it's bespoke integration per customer and the "works for anything" claim fails |
| P3 | Channel: web widget first; ingress/egress kept **outside** the core | channel-agnostic by construction; voice/WhatsApp later without core changes |
| P4 | Latency target: **warm turn p95 ≤ 400ms**, **deep turn with one tool ≤ 1.5s** | sets the caps for graph traversal budget and pre/post-filter thresholds |
| P5 | Tenant approves **effectful** L3 procedure promotions; read-only promotions auto-gate on evidence | keeps write paths under human control without throttling learning |
| P6 | SDK registration schema is **open for specification** | it is the product; disproportionate design effort justified |
| P7 | MVP: one tenant, arbitrary tool count, one authored flow, tier-0/1 hit rate as the headline metric | generality proven by tool arbitrariness, not tool count |

---

## 2. Locked decisions — tenant lifecycle

Tenant carries an explicit **mode** column; the executor reads it and gates behavior.

| Mode | Behavior |
|---|---|
| `bake` | onboarding. Tier-3 heavy. Sandbox + ledger replay. Tenant approves effectful procedures. High LLM budget, latency irrelevant. No live traffic. |
| `live` | production. Tier-0/1 target. Small exploration budget ε. Tier-3 is rare and **alarmable**. |
| `rebake` | triggered by registry invalidation. Affected procedures demoted; partial re-bake. |

This decision eliminates the cold-start problem: no tenant ever meets a real user with an empty procedure vocabulary.

---

## 3. Locked decisions — storage & isolation

| # | Decision | Rationale |
|---|---|---|
| S1 | **Per-tenant table names** for tenant-scoped data | Aelio DB's isolation unit is the table (single node, one engine, one WAL). Tenant boundary becomes structural — cannot leak by forgetting a filter. |
| S2 | System-owned tables stay **shared and read-only**: operation registry, prompt specs, system docs | avoids N-way duplication of the ability catalog |
| S3 | Table resolution behind exactly one function `resolveTable(tenant, logical_name)` | decision remains reversible |
| S4 | **Migration runner required from day one** | per-tenant tables make schema change an N×26 DDL operation |
| S5 | **Conditional write is being added to Aelio DB** — `insertRowIf(table, row, predicate)` and `updateRowIf(row_id, expected_version, patch)` + version column | unblocks four separate races at once; far cheaper than full transactions |
| S6 | Until CAS lands: **single-writer per tenant** — all writes for a tenant routed through one process | per-tenant tables already supply the shard key; serialization without DB support |
| S7 | Turn consistency = **saga**, not transaction. Idempotency by logical key or content hash, append-only audit, background reconciler | no multi-row transaction on the current API |
| S8 | Vector recall: rely on Aelio DB's native filtered vector search. Selective filter → exact pre-filter; loose filter → ANN post-filter with over-fetch | confirmed native, cost-model driven, with exact fallback on low recall |
| S9 | Graph in hot path only under budget: `maxDepth ≤ 3`, max frontier, max visited rows, max elapsed ms, tenant/customer filter, fallback to vector/text on budget exceed | 3-hop safe only at low fanout |

### What CAS unblocks (all the same missing primitive)

1. `Once{idem_key}` — atomic insert-if-absent
2. `Learn.Promote` — no double-promotion by concurrent sweepers
3. `Park` / scheduler leasing — survives multi-process
4. One active suspension per session

---

## 4. Locked decisions — execution model

| # | Decision | Rationale |
|---|---|---|
| E1 | **Determinism sandwich**: every LLM ability is deterministic prompt build → model → deterministic parse into closed vocabulary | makes an LLM call composable; contract is deterministic even though the core isn't |
| E2 | Four substrates: **pure / semantic / llm / effect** | cost-ordered composition; cheap deterministic filters first, model calls last |
| E3 | Uniform ability contract at **every** rung L0→L4 | caller cannot tell whether it invokes one primitive or a 100-step flow; this is what lets blocks grow into bigger blocks |
| E4 | Execution deterministic, **selection** never learned at the primitive level — the learnable object is the **edge** | narrow, safe self-evolving surface |
| E5 | Combinators (S0-A) are a **separate closed namespace** from pure ops (S0-B) | combinator type depends on contents; mixing breaks postcondition→precondition composition |
| E6 | S0-A kept small (~31 ops) | it is the branching factor for tier-2 compositional search |
| E7 | **Totality**: every op returns `Result<T, ReasonCode>`. Nothing throws, no ambiguous nulls | composed procedures otherwise inherit unhandled failure paths |
| E8 | **Bounded everything** — loops, repeats, retries, fan-out, recursion all carry declared ceilings | a learned procedure must not be able to hang a turn or drain tenant budget |
| E9 | Higher-order collection ops (`Map`/`Filter`/`Reduce`/`Find`/`Any`/`All`) live in **S0-A**, not S0-B | they take an `Op` argument — structurally combinators |
| E10 | `Now`, `Uuid`, `Random` are **effects**, recorded for replay | otherwise replay diverges |
| E11 | **`Sleep` does not exist** — only `Park{until}` | blocking sleep holds a worker for the whole OTP window |
| E12 | `Policy.Evaluate` is an **executor invariant**, wrapped structurally around every `Invoke.Call` and `State.ProposeTransition` | the composer must not be able to omit it |
| E13 | `Invoke` split into **four** — `Call` / `Signature` / `Extract` / `Interpret` | binding becomes learnable, extraction cacheable, ledger brackets `Call` alone |
| E14 | `Understand.SplitClauses` needs a **pure pre-gate** (word count, clause markers, length) | it sits at the top of every turn; its cost is otherwise paid unconditionally, including on "Hi" |
| E15 | Depth is decided **before** it is paid for: classify at the cheapest substrate that can decide | shallow / boundary / deep |
| E16 | `Understand.ResolveReference` returns `Ambiguous[..]`, never a best guess | confident-wrong resolution is the worst failure mode in a business context |
| E17 | Generic + input-oriented replies → `Express.Template`; output-oriented + banter → `Express.Synthesize` | free, auditable, legally reviewable by the tenant |
| E18 | `Express.Synthesize` is the **only** open-output ability, and it is terminal | nothing consumes its output but the user |
| E19 | `Judge.Confidence` is a first-class ability, not a hidden field | escalation thresholds become measurable and tunable per ability rather than magic constants |
| E20 | Turn lock **per user**; declared conflict rule for concurrent slot writes | proactive nudge firing mid-flow is a real interleaving risk |

---

## 5. Locked decisions — flows, procedures, learning

| # | Decision | Rationale |
|---|---|---|
| F1 | **Both** authored flows and learned procedures exist. Flows are rails; procedures cover the long tail | tenant can point at `convox.flows.signup` and know exactly what happens |
| F2 | Flow **activation** is deterministic: hard preconditions (state, policy) filter first; embedding match breaks remaining ties only | activation is where all the flow's nondeterminism concentrated after the body was frozen |
| F3 | A flow step declares a **postcondition** (what must be true), not an implementation | this is the seam: any L2 block or promoted L3 procedure satisfying it is admissible; learning improves flows from underneath while ordering stays frozen |
| F4 | `learnable: false` for auth / payment / destructive flows — **structurally barred** from reordering proposals | compliance surface |
| F5 | Under `learnable: false`, learning may still tune extraction thresholds, repair wording, retry timing | implementation, never sequence |
| F6 | Preemption is a **per-flow declared property**; default `Hold` (answer digression, return to pending step) | mid-flow abandonment is the failure mode that costs the tenant money |
| F7 | Flows declare an **escape**: `Fallback(flow)` / `FreeRange` / `Escalate` | rails must not silently improvise when reality deviates |
| F8 | Resumed flows **re-check policy and state**; never trust pre-suspension authorization | security hole otherwise |
| F9 | Tier ladder: **0** exact σ hit → **1** near hit within ε with margin → **2** type-directed composition over the ability graph → **3** LLM proposes a path | tier 2 is the difference between a cache and a vocabulary |
| F10 | Tier 3 output is a **constrained ordered path over declared abilities**, never prose; type-checked before any step executes | unsatisfiable proposals die structurally |
| F11 | Promoted procedures are **immutable**; improvements create v2 alongside v1, both retrievable, ranked by evidence | gives rollback when "better" loses under real traffic |
| F12 | **Asymmetric gating**: slow to promote (≥N clean observations clearing success-rate + cost thresholds), instant to suspend on mismatch or dependency change | cheap to re-earn, expensive to be wrong |
| F13 | **Per-step credit assignment**, not per-path | a six-step failure demotes the failing step, not the surrounding five |
| F14 | **Exploration budget ε**: a fraction of tier-0 hits run the runner-up and record comparative outcome | without it, every procedure freezes at the first thing that worked |
| F15 | Success signal must have teeth — tool non-error, **repair turns**, flow terminal state, abandonment, latency, cost. Never an LLM self-grade | a model asked "was that good?" is a soft grader |
| F16 | `tool_deps` on every ability; SDK re-registration **cascades invalidation** | most likely source of production embarrassment: confidently retrieved, silently broken procedures |
| F17 | Stable procedures may be **promoted to candidate flows** and surfaced to the tenant for approval | the loop that makes this a learning organism rather than a config tool |
| F18 | Situation key σ fields must be **bucketed** before entering the key (`turn_index: first/early/established`, `last_seen: never/recent/lapsed/dormant`) | raw precision gives every situation a unique σ and a 0% tier-0 hit rate |
| F19 | **Personality must not enter σ** | otherwise a tone change invalidates every learned procedure the tenant has |

---

## 6. Locked decisions — tool call lifecycle

| # | Decision | Rationale |
|---|---|---|
| T1 | Parameter spec carries `source: user \| slot \| state \| env \| derived \| tool_output \| const` | most arguments should never be asked for; without this the binder asks for things the system already knows — the commonest way agentic systems feel stupid |
| T2 | Parameter spec carries `sensitivity: none \| pii \| secret` | keeps OTPs and tokens out of the ledger and out of prompts |
| T3 | Parameter spec carries `constraint`, `repair` (normalize fn), `depends_on`, `prompt_hint`, `default` | binder becomes fully deterministic; only the genuine residual escalates to extraction |
| T4 | ToolSpec declares **capability tags**, **success semantics**, and **expected continuations** | "understands OTP will be sent" is a schema read, not an inference; this is what keeps step-1 tool resolution out of judgment |
| T5 | **Response signature** computed on the raw response, *before* any cleaning | sanitizing first normalizes away the very difference that detects a tenant API change |
| T6 | Signature → hash → promoted **extraction plan** (path + validator), applied as a pure op | zero model, microseconds, after promotion |
| T7 | **Signature mismatch → never guess.** Fall back to `Invoke.Interpret`, open a new proposal branch | a cached extractor applied to a response it wasn't derived from is silent corruption — worse than an error |
| T8 | **Learn the structure, declare the meaning** | `{"code":"434543"}` is otherwise indistinguishable from a discount code |
| T9 | Response **role** classified: `data` / `effect_confirmation` / `continuation` / `error` | drives different executor behavior |
| T10 | **Error signatures are first-class**, mapped to closed `ReasonCode` + `recovery: Retryable \| NeedsRepair \| Terminal \| NeedsEscalation` | this is what turns the executor from brittle into resilient |
| T11 | Ledger brackets `Invoke.Call` **alone** | the only line where a crash actually loses information |
| T12 | Retry admissible **only** for idempotent bodies; enforced structurally | |

---

## 7. Locked decisions — sanitation, prompts, verification

| # | Decision | Rationale |
|---|---|---|
| C1 | Pipeline: `raw → Sig.Compute → Sig.Match → Extract → Clean → Redact → Project → Enrich(provenance) → TypedRecord` | |
| C2 | Cleaned records carry **provenance** (tool, call, receipt) | synthesis can attribute; `Judge.Groundedness` has something to check against; a clean record with no provenance is unverifiable |
| C3 | Redaction happens **before** anything enters an LLM context window | |
| C4 | A prompt is a **versioned dependency**, not a string: `PromptSpec{frame, slots, output_contract, examples, budget, truncation_order, hash}` | |
| C5 | Prompt assembly is deterministic — same inputs, byte-identical prompt | required for replay |
| C6 | **`prompt_hash` is part of the ability version.** Prompt changes cascade invalidation exactly like `tool_deps` changes | a prompt change alters behavior, staling every calibrated threshold and every dependent procedure. Currently missing from all artifacts. |
| C7 | **Truncation order declared**, not implicit | otherwise the same situation produces different prompts under different context loads — silent nondeterminism |
| C8 | Deterministic verification (`Judge.Postcondition`) runs on **every** step; semantic verification (`Judge.Groundedness`, `Judge.Verify`) runs **only** at terminal synthesis and tier-3 supervised paths | cheap check everywhere, expensive check where being wrong is user-visible |

---

## 8. Locked decisions — semantic term resolution

The "avaricious ≈ greedy" problem, and the trap in it.

| # | Decision | Rationale |
|---|---|---|
| M1 | Term resolution is **anchored to declared attributes**, never free-floating similarity over text | **antonyms are distributionally close** — "greedy" and "generous" occupy near-identical contexts. Pure nearest-neighbour on "avaricious" can confidently return the *least* demanding clients. |
| M2 | Attribute declares `anchors` (tenant-declared) + `learned` (promoted synonyms) + **`polarity`** | without polarity, "most avaricious" and "least avaricious" are the same neighbourhood |
| M3 | Thin margin or uncertain polarity → **confirm with the user once**, then write the synonym as a graph edge, promoted after N confirmations | free forever after |
| M4 | Term resolution outputs a **query-plan fragment** `{attribute, direction, limit}`, not a similarity score | "ten most X" requires a rankable, sortable field — a topical filter is insufficient |

---

## 9. Locked decisions — sandbox & validation

Three tiers. Only the first two are automatic.

| Tier | Method | Notes |
|---|---|---|
| 1 — static | type-check the path: postcondition→precondition unification, slot sourcing, policy admissibility, loop bounds | free, instant, kills most bad proposals |
| 2 — ledger replay | run the proposed procedure against **recorded** tool responses instead of live tools | the ledger is a free regression corpus; **build this before anything else in the learning loop** |
| 3 — live shadow | read-only paths only; run alongside promoted path, serve promoted, log disagreements | |

**Effectful steps are never validated by execution.** They pass on type + policy + tenant approval, and enter production behind the exploration budget with a hard cap.

**Product consequence:** tools may declare a **dry-run mode**. Tenants who provide it get faster procedure learning on write paths. This is a real incentive to document in the SDK.

---

## 10. Locked decisions — document I/O

Capability confirmed needed. Shape changed: **virtual document store, not filesystem.**

| # | Decision | Rationale |
|---|---|---|
| D1 | No `read(path)` / `write(path)` primitive | LLM-proposed procedures executing in a multi-tenant server must not be able to name a filesystem path |
| D2 | `ns` is a namespace tuple `(tenant_id, scope, class)` resolved by the runtime | traversal structurally impossible |
| D3 | Scopes: `tenant` / `user` / `session` (TTL'd) / `system` (read-only, ours) | |
| D4 | Classes: `knowledge` (tenant uploads: rules, laws, product docs) / `plan` (session scratchpad) / `note` (durable per-user) / `artifact` (generated output) / `spec` (ability docs the composer reads) | |
| D5 | Docs **chunked and embedded on write**; `Doc.Search` and `Doc.Section` are the primary access, not `Doc.Read` | reading a 40-page policy doc into a prompt burns the whole context budget |
| D6 | `Doc.Patch` operates on a section, not whole-file rewrite | concurrent writes to different sections don't clobber |
| D7 | Per-tenant quotas on doc count, bytes, write rate | a runaway `Loop` writing docs is a cheap DoS on storage cost |
| D8 | `Doc.Write` is effectful — policy-gated, idempotency-keyed, never sandbox-validated by execution | |
| D9 | `spec` scope is how the cold-path LLM learns what abilities exist | avoids cramming the dictionary into every prompt |

---

## 11. Storage amendments required

Beyond the target model already in `HARNESS_AELIO_DB_DATA_MODEL.md`.

### Amendments to planned tables

| Table | Add |
|---|---|
| `harness_operations` | `cost_class`, `substrate`, `preconditions`, `postconditions`, `tool_deps`, `prompt_hash`, `idempotent`, `effectful`, `suspendable`, `sensitivity` |
| `harness_policies` | `condition_json` must conform to the **closed predicate grammar** (§12) — otherwise it is tenant-supplied code being executed |
| all tenant tables | `version` column, for `updateRowIf` optimistic concurrency |

### New tables required

| Table | Purpose | Key |
|---|---|---|
| `harness_procedures` | L3 learned compositions. Distinct from `harness_operations`: tenant-learned, evidence-gated, `situation_key` vector, `status`, `supersedes`, `provenance`. **Operations are declared; procedures are earned.** | `tenant + procedure_id + version` |
| `harness_signatures` | response `ShapeSig` → hash → extraction plan → promotion state | `tenant + tool_id + sig_hash` |
| `harness_proposals` | unpromoted candidate ledger; where evidence accumulates before promotion fires | `tenant + proposal_id` |
| `harness_prompts` | versioned `PromptSpec`; `prompt_hash` referenced by `harness_operations` | `prompt_id + version` (shared scope) |
| `harness_docs` | virtual document store | `tenant + scope + class + key + version` |
| `harness_doc_chunks` | chunk-level embeddings for `Doc.Search` / `Doc.Section` | `doc_id + chunk_index` |

---

## 12. Closed predicate grammar (policy)

Predicates may reference **only**: `state`, `role`, `slot`, `env` (time/locale/channel), `tool`, `consent`, `budget`, `evidence`, `flow_context`.

Operators: comparison, membership, range, `and` / `or` / `not`.

**No function calls, no loops, no I/O.** Closing this grammar is what prevents policy from becoming tenant-supplied code the server executes.

---

## 13. Evaluation harness — build before the learning loop

| # | Method | Purpose |
|---|---|---|
| V1 | **Golden traces** — ~30 hand-written expected step sequences, diffed against actual | shows exactly which step diverged; the single most valuable early artifact |
| V2 | **LLM-call assertions** as hard tests: `assert llm_calls("Hi", warm) == 0` | prevents model calls creeping back into hot paths; without it you find out from the bill |
| V3 | **Tier-0/1 hit rate** as the headline metric | low = fingerprints too specific = a cache with no hits |
| V4 | **Replay** — deterministic steps must produce byte-identical results on re-run | catches non-determinism leaking where it shouldn't |
| V5 | **Shadow mode** — run cheap and expensive paths side by side, serve cheap, log disagreement | the only honest way to set escalation thresholds |
| V6 | **Fault injection** — garbage responses, timeouts, changed shapes | the dangerous failure is not a crash but a stale extraction plan that succeeds with the wrong field |
| V7 | **Escalation rate per ability** as health signal | rising rate = semantic layer has drifted from tenant reality |

---

## 14. Determinism budget (target)

| Tier | LLM calls |
|---|---|
| shallow | 0 |
| boundary | 1 (synthesis only) |
| deep, tier 0/1 | 1 (terminal synthesis only) |
| deep, tier 2 | 1–2 |
| deep, tier 3 | 3–5 (should trend toward 0% of `live` traffic) |

---

## 15. Still open

Carried forward; not blocking the architecture document but must be resolved during implementation.

1. **Memory block taxonomy** — episodic / factual / entity / procedural / open-loop; distinct write rules, decay, retrieval shape. Entity identity resolution across turns is the hard sub-problem.
2. **Evidence signal set** — exact enumeration of what feeds `Judge.Outcome`.
3. **Nudge / direction semantics** — the `direction` half of `StateSpec` is structurally specified, policy unwritten.
4. **Cross-tenant generalization** — default no. Per-tenant tables make it structurally hard, which is consistent. Genuine strategic question left open.
5. **Calibration harness** — every `⇄` pair needs empirical thresholds per ability per tenant.
6. **Multi-clause conflict** — resolution order when `SplitClauses` yields contradictory clauses.
7. **Exploration rate ε** — value, decay schedule, per-flow opt-out.
8. **Locale/currency formatting** — an ability (needs locale data), not a pure op.
9. **Whole-system failure** — degradation ladder when Aelio DB, the LLM provider, or the tenant's API is down. A conversational interface that hangs is worse than one that says it can't reach the system.
10. **Data lifecycle / deletion** — GDPR/DPDP. A user's turns, facts, entities, embeddings are deletable; what about a procedure learned partly from their behavior?

---

## 16. Proposed architecture-document outline

Twelve parts. Written toward implementability, not aspiration.

| Part | Contents |
|---|---|
| I | Problem, product shape, core bet, non-goals |
| II | Substrates, the determinism sandwich, the uniform contract, the L0→L5 ladder |
| III | Instruction set: S0-A combinators, S0-B pure ops, S0-C effects (from the dictionary) |
| IV | L1 abilities, family by family, with escalation pairs and calibration |
| V | **SDK registration schema** — ToolSpec, parameter spec, output semantics, capability tags, continuations, dry-run. *The keystone chapter.* |
| VI | Tool call lifecycle: binding, invocation, signatures, extraction plans, error taxonomy, sanitation |
| VII | Prompt system: PromptSpec, deterministic assembly, budget, truncation, hash-as-version, invalidation cascade |
| VIII | Retrieval: recall shapes, filtered vector strategy, graph budgets, term resolution with polarity anchoring |
| IX | Flows, states, policies, personality; activation, preemption, deviation, suspension, resume |
| X | Learning: tier ladder, situation keys and bucketing, proposal ledger, promotion gates, credit assignment, exploration, sandbox tiers, invalidation |
| XI | Data model: table map, per-tenant resolution, saga write path, CAS usage, retention, PII |
| XII | Operations: tenant lifecycle modes, evaluation harness, observability, degradation, migration plan, acceptance criteria |

Appendices: worked traces (`"Hi"` cold and warm, `login`, `"ten most avaricious clients"`), ReasonCode taxonomy, golden-trace format.
