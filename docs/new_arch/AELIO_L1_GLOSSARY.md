# Aelio L1 — Normative Abilities Glossary

**Status:** normative living contract (v0.1)
**OS role:** system services ([`AELIO_AGENT_OS.md`](./AELIO_AGENT_OS.md))
**Built on:** L0 only via Op trees + `Call{id}` ([`AELIO_L0_GLOSSARY.md`](./AELIO_L0_GLOSSARY.md))
**Supersedes for L1 decisions:** `aelio_dsl_dictionary.md` §4 where this doc conflicts
**Rust:** `aelio-os/crates/aelio/src/abilities/`

---

## How to read

| Field | Meaning |
|---|---|
| **OS role** | Which system-service job this ability performs |
| **Sub** | `P` pure · `S` semantic · `L` llm · `E` effect · `⇄` escalate on thin margin |
| **Decision** | `keep` · `defer` · `demote` · `keep-as-L2` |
| **Status** | `normative` · `aspirational` · `implemented` · `partial` |

### L1 rules (locked)

1. Abilities have **cost**, typed **fail** codes, and often **multiple substrates** — that is the line against L0-B.
2. Callers use the **uniform contract**; they cannot tell primitive vs deep composition.
3. No new L0 combinators for domain behavior — express as `Call` + L0-A.
4. Personality constrains **Express** only — never situation keys or policy.
5. **Policy** wraps effectful Calls as an **executor invariant**, not only as a composed step.

---

## 1. Sense — cockpit / procfs (read-only awareness)

**OS role:** Assemble a per-turn snapshot from clock, request, and durable PCBs. Sense **does not store**.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Sense.Env` | → `{now, tz, locale, session_age, turn_index, channel}` | P | keep | normative / partial |
| `Sense.Budget` | → `{tokens_left, ms_left, calls_left, cost_spent}` | P | keep | normative / partial |
| `Sense.Self` | → `{escalation_rate, recent_failures, degraded_abilities, cache_hit_rate}` | P | keep | normative / partial |
| `Sense.Session` | → `{open_loops, active_flow, pending_step, last_seen, parked_at?}` | P | keep | normative / partial |
| `Sense.Tenant` | → `{tenant_id, plan, feature_flags, tool_count}` | P | keep | normative / partial |
| `Sense.Location` | `{consent_ref} → Option<Geo>` | E | keep | normative / aspirational |

**Notes:** Rising `Sense.Self.escalation_rate` = semantic drift signal. Session fields hydrate from `flow_instances` / loops / session rows — not a Sense table.

**L0 build:** `Call("Sense.*")` at turn start; no domain combinators.

---

## 2. Understand — utterance → structure

**OS role:** Parse language into typed control labels (clauses, depth, intent, slots). Does not Invoke or Remember.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Understand.SplitClauses` | `{utterance} → List<Clause>` | L (+ pure pre-gate) | keep | normative / partial |
| `Understand.ClassifyDepth` | → `shallow\|boundary\|deep` + margin | S⇄L | keep | normative / partial |
| `Understand.ClassifyIntent` | `{utterance, label_set} → Label + margin` | S⇄L | keep | normative / partial |
| `Understand.ClassifyReplyType` | → Generic\|InputOriented\|OutputOriented\|Banter | S⇄L | keep | normative / partial |
| `Understand.Extract` | `{utterance, schema} → TypedRecord \| Partial` | S⇄L | keep | normative / partial |
| `Understand.ResolveTemporal` | → Interval \| Unresolved | S⇄L | keep | aspirational / thin |
| `Understand.ResolveReference` | → Id \| Ambiguous \| Unresolved | S⇄L | keep | partial |
| `Understand.DetectRepair` | → Normal\|Repair\|Frustration\|Abandon | S⇄L | keep | partial |
| `Understand.DetectConsent` | → Affirm\|Deny\|Unclear | S⇄L | keep | partial |
| `Understand.Tokenize` | → List\<Token\> | S | keep | aspirational |
| `Understand.DetectLanguage` | → LangTag + margin | S | keep | aspirational |
| `Understand.RewriteQuery` | → SearchString | S⇄L | keep | aspirational |

**Rules:** Ambiguity is a typed outcome (never silent wrong id). Depth heuristics may bootstrap; long-term intent comes from **declared capability labels**. Hardcoded vertical nouns in depth lists must shrink over time.

**L0 build:** `Fallback` / `Budget` + `Call` for S⇄L; `Branch` on depth.

---

## 3. Recall — filesystem read API

**OS role:** Retrieve evidence from Aelio DB spaces. Does not choose the turn path (Learn does).

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Recall.Exact` | `{space, key} → Option<Record>` | P | keep | normative / partial |
| `Recall.Lexical` | `{space, terms, k, filter} → List<Hit>` | S | keep | normative / partial |
| `Recall.Semantic` | `{space, query: Vector, k, filter} → List<Hit>` | S | keep | normative / partial |
| `Recall.Graph` | `{node, axis, depth, limit} → List<Node>` | S | keep | aspirational / thin |
| `Recall.Neighbors` | `{node, edge_type, limit}` | S | keep | aspirational |
| `Recall.Path` | `{from, to, max_hops}` | S | keep | aspirational |
| `Recall.Fuse` | `{sets, weights, strategy} → List<Hit>` | P | keep | normative / partial |
| `Recall.Rerank` | `{candidates, query}` | S⇄L | keep | aspirational |
| `Recall.Embed` | `{text, model_ref} → Vector` | S | keep | normative / partial |
| `Recall.EmbedBatch` | `{texts, model_ref}` | S | keep | aspirational |

**Recall shapes** (HybridBroad, FilterThenRank, …) are **L2 blocks**, not L1 primitives — selected deterministically from query shape.

**Turn rule:** Recall answers knowledge only when claims exist **and** intent has no catalogued tool; tools win.

**L0 build:** `Call("Recall.*")` + optional L0-B over hit lists for Fuse weights.

---

## 4. Judge — honesty / admission control

**OS role:** Gate escalation, completeness, postconditions, groundedness. Not a chat model for policy.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Judge.Confidence` | `{candidates} → margin, entropy, calibrated_p` | P | keep | normative / partial |
| `Judge.Sufficient` | `{slots, requirement} → Complete \| Missing` | P | keep | normative / partial |
| `Judge.Postcondition` | `{state, invariant} → Ok \| Violation` | P | keep | normative / partial |
| `Judge.Admissible` | `{ability, ctx} → Ok \| Denied` | P | keep | partial |
| `Judge.Verify` | `{claim, evidence} → Bool + ReasonCode` | L | keep | aspirational |
| `Judge.Groundedness` | `{utterance, evidence}` | L | keep | partial |
| `Judge.Outcome` | `{turn_record} → OutcomeScore` | P | keep | partial |

**L0 build:** Prefer pure Judges as Call; escalate Verify/Groundedness behind Budget.

---

## 5. Bind — argument marshalling

**OS role:** Fill tool params from state/slots/env before asking the user.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Bind.Resolve` | `{param_spec, sources} → Value \| NeedsUser` | P | keep | normative / partial |
| `Bind.ResolveAll` | `{tool_spec, sources} → {bound, residual}` | P | keep | normative / partial |
| `Bind.Normalize` | `{value, repair_id} → Value \| Violation` | P | keep | normative / partial |
| `Bind.Validate` | `{args, tool_spec} → Ok \| Violations` | P | keep | normative / partial |

**Rules:** `source` on ParamSpec is load-bearing. Repair ids reference **registered** normalizers (see L0 demotion of NormalizePhone to registry).

---

## 6. Invoke — external tool syscall

**OS role:** The only external world-mutation Call for tools. Ledger brackets this line.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Invoke.Call` | `{tool, args, idem_key} → Raw \| ToolError` | E | keep | normative / partial |
| `Invoke.Signature` | `{raw} → SigHash` | P | keep | normative / partial |
| `Invoke.Extract` | `{raw, plan} → Typed \| SigMismatch` | P | keep | normative / partial |
| `Invoke.Interpret` | `{raw, tool_spec} → Typed + plan` | L | keep | partial (cold) |
| `Invoke.ClassifyError` | `{error, taxonomy} → ReasonCode + Recovery` | S⇄L | keep | partial |

**L0 pattern:** `Guard(Policy)` → Bind* → `Once` → `Tee(LedgerAppend)` → `Call(Invoke.Call)` → Sig*.

---

## 7. Sig — response-shape registry

**OS role:** Learn structural extractors for tool JSON; never guess on mismatch.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Sig.Compute` | `{raw} → ShapeSig` | P | keep | normative / partial |
| `Sig.Hash` | `{shape} → SigHash` | P | keep | normative / partial |
| `Sig.Match` | `{hash, registry} → Option<ExtractionPlan>` | P | keep | normative / partial |
| `Sig.Propose` | `{raw, interpreted} → ExtractionPlan` | P | keep | partial |
| `Sig.Classify` | → Data\|EffectConfirmation\|Continuation\|Error | P | keep | partial |

**Rule:** Mismatch → Interpret + new proposal branch; never apply wrong cached plan.

---

## 8. Express — tty / user surface

**OS role:** Sole open channel to the human. `Synthesize` is terminal (output not control).

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Express.Template` | `{id, bindings} → Utterance` | P | keep | normative / partial |
| `Express.Ask` | `{missing_slot, hint, attempt_n}` | P⇄L | keep | normative / partial |
| `Express.Confirm` | `{action, args, personality}` | P⇄L | keep | partial |
| `Express.Clarify` | `{ambiguity, personality}` | P⇄L | keep | partial |
| `Express.Synthesize` | `{evidence, personality, constraints}` | L | keep | normative / partial |
| `Express.Apologize` | `{reason_code, recovery, personality}` | P⇄L | keep | partial |
| `Express.Style` | `{utterance, personality}` | L | keep | aspirational |

**Rule:** Personality may enter Express only.

---

## 9. Remember — durable write API

**OS role:** Persist turns, facts, entities, open loops. Effectful; policy-sensitive.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Remember.WriteTurn` | `{turn_record}` | E | keep | normative / partial |
| `Remember.WriteFact` | `{fact, subject, confidence, provenance}` | E | keep | partial |
| `Remember.WriteEntity` | `{entity, attrs, aliases}` | E | keep | aspirational |
| `Remember.LinkEntity` | `{a, b, edge_type}` | E | keep | aspirational |
| `Remember.OpenLoop` | `{loop_spec} → LoopId` | E | keep | partial |
| `Remember.CloseLoop` | `{loop_id, resolution}` | E | keep | partial |
| `Remember.Forget` | `{selector, reason}` | E | keep | partial |
| `Remember.Decay` | `{policy}` | E | keep | aspirational |
| `Remember.Consolidate` | `{window} → Summary` | L | keep | aspirational |

**Rule:** Do not open loops on bare greetings.

---

## 10. State & Policy — lifecycle + MAC

**OS role:** Where the user is in the product graph; what actions are allowed.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `State.Read` | `{user} → StateNode` | P | keep | normative / partial |
| `State.ProposeTransition` | `{user, target, evidence}` | E | keep | normative / partial |
| `State.Reachable` | `{from} → List<StateNode>` | P | keep | partial |
| `State.Direction` | `{current} → Option<Target>` | P | keep | partial |
| `Policy.Evaluate` | `{subject, action, ctx} → Allow \| Deny` | P | keep | normative / partial |
| `Policy.Explain` | `{decision} → PredicateTrace` | P | keep | aspirational |

**Rule:** Policy is never an LLM. Resume after Park **re-checks** policy/state.

---

## 11. Registry — package database / linker view

**OS role:** Discover tools, flows, procedures, capabilities; invalidate dependents.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Registry.LookupTool` | `{tenant, selector}` | P | keep | normative / partial |
| `Registry.LookupFlow` | `{tenant, selector}` | P | keep | partial |
| `Registry.LookupProcedure` | `{tenant, situation_key}` | P | keep | partial |
| `Registry.Capabilities` | `{tenant}` | P | keep | partial |
| `Registry.Reachable` | `{tenant, state, policy_ctx}` | P | keep | partial |
| `Registry.Version` | `{ref}` | P | keep | aspirational |
| `Registry.Invalidate` | `{tool_id} → invalidated refs` | E | keep | partial |

---

## 12. Learn — installer / routine factory

**OS role:** Situation key, tier retrieve, compose/propose paths, promote/demote.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Learn.SituationKey` | → σ (bucketed; no personality) | P | keep | normative / implemented |
| `Learn.LookupTier` | → Tier0\|1\|2\|3 + candidates | P | keep | normative / partial |
| `Learn.Compose` | bounded backward search → Option\<Path\> | P | keep | normative / partial |
| `Learn.ProposePath` | `{σ, ability_set} → Path` | L | keep | normative / partial |
| `Learn.TypeCheck` | `{path} → Ok \| Unsatisfiable` | P | keep | normative / partial |
| `Learn.ScoreStep` | `{step_record} → StepScore` | P | keep | partial |
| `Learn.Attribute` | `{path_record, outcome} → credits` | P | keep | partial |
| `Learn.Propose` | `{candidate} → ProposalId` | E | keep | partial |
| `Learn.Promote` | `{proposal} → Ok \| GateNotMet` | E | keep | partial |
| `Learn.Demote` | `{ref, reason}` | E | keep | partial |
| `Learn.Explore` | `{σ, budget} → Option\<AlternatePath\>` | P | keep | aspirational |

**Rules:** ProposePath never emits prose — only ordered ability paths. Promote slow / Demote fast. Paths are L0 Op trees over declared abilities.

---

## 13. Proactive — outbound scheduler jobs

**OS role:** Daemon path for open loops / nudges without inbound message.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Proactive.EvaluateLoops` | `{user} → List<Candidate>` | P | keep | aspirational / thin |
| `Proactive.Decide` | `{candidates, policy_table} → Option<Action>` | P | keep | aspirational |
| `Proactive.Cadence` | `{user} → window, jitter` | P | keep | aspirational |
| `Proactive.BudgetCheck` | `{user, action}` | P | keep | aspirational |
| `Proactive.Send` | `{action, idem_key} → Receipt` | E | keep | partial (channel send exists at edge) |
| `Proactive.Suppress` | `{user, reason, ttl}` | E | keep | aspirational |

**Rule:** Deterministic Decide table **before** any model. Uses L0-C `Schedule` / `Park` / `Once`.

---

## 14. Observe — admin / debug plane

**OS role:** Inspect traces, proposals, metrics; human approve promotions.

| Ability | Contract | Sub | Decision | Status |
|---|---|---|---|---|
| `Observe.Trace` | `{turn_id} → TurnTrace` | P | keep | partial |
| `Observe.Explain` | `{decision} → Explanation` | P | keep | aspirational |
| `Observe.PendingProposals` | `{tenant}` | P | keep | partial |
| `Observe.Approve` | `{proposal_id, actor}` | E | keep | partial |
| `Observe.Metrics` | `{tenant, window}` | P | keep | aspirational |

---

## 15. Family order in a reactive turn

```text
Sense → (flow gate) → Understand → Recall? → Learn(tiers)
  → Bind → Policy → Invoke/Sig → Judge → Express → Remember/State
```

Proactive and Observe run off the hot reactive path (scheduler / admin).

---

## 16. L1 sufficiency vs L0

All families above are expressible as **L0 trees + Calls** under the frozen L0 glossary. No domain L0 combinators required. Gaps are **implementation depth**, not missing kernel opcodes.

---

## 17. Change control

1. New L1 ability: OS role + contract + substrate + status row in this file.
2. Effectful abilities must declare Policy interaction.
3. Learning abilities must declare promote/demote asymmetry.
4. Code cites ids like `L1.Recall.Semantic`, `L1.Learn.Promote`.
