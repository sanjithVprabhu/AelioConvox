# Aelio / Convox — The Architecture, Explained

A narrative walkthrough of what we are building, why each piece exists, and exactly what happens inside the machine when a person types something.

Companion artifacts:
- `aelio_dsl_dictionary.md` — the complete instruction set
- `aelio_decision_ledger.md` — every decision with its rationale
- `HARNESS_SUNJET_DATA_MODEL.md` — the storage spine

---

# Part I — What we are building

## 1.1 The inversion

MCP standardizes tool exposure **to** a model that the user already has. You bring Claude, Claude connects to your Jira, Claude reads your tickets.

Aelio points the same idea the other way. It standardizes conversational exposure **of** a product to that product's own end users. A CRM company installs our SDK, registers their tools, and their customers get a conversational interface over the entire product — without the CRM company building an agent, hiring an AI team, or handing anyone an API key.

```
        MCP                              AELIO
  ┌──────────────┐                 ┌──────────────┐
  │  user's LLM  │                 │  SaaS product│
  └──────┬───────┘                 └──────┬───────┘
         │ connects to                    │ exposes itself via
         ▼                                ▼
  ┌──────────────┐                 ┌──────────────┐
  │ SaaS product │                 │ Aelio server │
  └──────────────┘                 └──────┬───────┘
                                          │ serves
                                          ▼
                                   ┌──────────────┐
                                   │ SaaS's users │
                                   └──────────────┘
```

The SaaS company pays. Their customers talk. That makes this B2B2C, which is why multi-tenancy is structural rather than a deployment footnote — tenant A's tools, personality, states, policies, and learned behavior must never be reachable from tenant B's turn.

## 1.2 What the server owes on every turn

Five things, every single turn:

1. **How to talk** — the tenant's personality, plus this specific user's history and register.
2. **What they mean** — intent, including references back into prior conversation ("the one I mentioned yesterday," "same as last time").
3. **What's permitted** — the user's lifecycle state and the policies gating it.
4. **What to invoke** — the right tool, correctly bound, at the right moment.
5. **What changed** — write-back to state, flows, memory, and the audit ledger.

## 1.3 The objective that constrains everything

**Vertical-agnostic.** No pre-coded verticals. A CRM, a logistics platform, a clinic booking system, a lending app — all should work with zero bespoke code.

That single requirement forces almost every other decision in this document, because it means the system can never assume it knows what a tool does. It has to work it out from what the tenant declared, plus what it has observed.

---

# Part II — The central bet

## 2.1 The LLM is not the router

The obvious way to build this is: give an LLM the tool list, let it decide what to call, let it decide what to say. That approach is fast to demo and impossible to sell to a company whose customer records are on the line.

Our bet is the opposite:

> **Retrieval and selection are deterministic. The model synthesizes, speaks, and — only when the system has never seen a situation before — proposes a path over a typed set of abilities.**

Formally: **the LLM is a proposal heuristic over a typed ability set, and the type system is the verifier.** This is type-directed program synthesis with a language model standing in for exhaustive search. The model guesses; the types check.

That reframing answers a question that otherwise requires judgment. "Does this result go to the user, or into another operation?" is not a decision — it's whether the output type unifies with `Express.Synthesize`'s input or with some other ability's precondition. The type checker answers it, deterministically, every time.

## 2.2 Why this matters commercially

Three consequences fall out, and they are the product:

- **Auditability.** Every step is a typed function with recorded inputs and outputs. A tenant can be shown exactly why the system did what it did.
- **Cost.** A warm turn costs zero to one LLM calls instead of five to fifteen.
- **Latency.** Target: warm turn p95 ≤ 400ms, deep turn with one tool ≤ 1.5s. Unreachable if a model is in the routing loop.

---

# Part III — The three substrates

Everything the system can do sits in one of four places, and knowing which one is what makes cost reasoning possible.

| Substrate | Speed | Cost | Reliability | Analogy |
|---|---|---|---|---|
| **Pure** | microseconds | zero | exact | arithmetic |
| **Semantic** (embeddings) | milliseconds | near-zero | unreliable, hard to trust | unconscious reflex |
| **LLM** | seconds | tokens | more reliable, never deterministic | deliberate thought |
| **Effect** | varies | varies | external | acting on the world |

The interesting observation — and this was the key insight in the design — is that **many capabilities exist in more than one substrate**. Extracting a phone number from a sentence can be done by a regex (pure), by an embedding-based extractor (semantic), or by an LLM. Same contract, three implementations, three cost profiles.

## 3.1 The escalation rule

Wherever an ability has both a cheap and an expensive implementation, we mark it `⇄` and it obeys one rule:

```
run cheap implementation
  → Judge.Confidence → {margin, entropy}
  → if margin < θ (calibrated per ability, per tenant)
       → escalate to LLM implementation
```

**Confidence is not raw similarity.** Raw similarity is what fails. It's the margin between the top two candidates plus the entropy of the distribution, calibrated empirically against real outcomes. Uncalibrated escalation gives you one of two failures: escalate always (the semantic layer is decoration, you pay LLM prices for everything) or escalate never (fast and confidently wrong).

The escalation rate per ability is also the single best health metric in the system. **When escalation on an ability starts rising, the semantic layer has drifted from the tenant's reality** — their data changed, their vocabulary changed, their users changed. It's an early warning that fires before anything visibly breaks.

## 3.2 The determinism sandwich

This is how an LLM call becomes composable at all.

```
     deterministic prompt build
              ↓
      ┌───────────────┐
      │  the model    │  ← the only nondeterministic part
      └───────────────┘
              ↓
   deterministic parse into a CLOSED vocabulary
```

You are never "calling the LLM." You are calling a typed function whose implementation happens to involve a model:

```
Understand.ResolveTemporal(utterance, anchors) -> Interval | Unresolved
```

The middle is stochastic. The **contract is not**. The output space was defined in advance, and anything outside it fails loudly rather than creatively. That's what allows a model call to sit inside a composition without poisoning everything downstream.

---

# Part IV — The ladder

Everything in the system is built from the layer beneath it, and every layer exposes the identical contract.

```
L5  Lifecycle       states, policies, personalities, activation
                    ── the world the tenant declares
L4  Flows           authored rails.  signup, checkout, escalate
                    ── ordering frozen; implementation open
L3  Procedures      LEARNED compositions.  situation → path → outcome
                    ── earned through evidence, gated by promotion
L2  Blocks          fixed compositions we author once
                    ── recall shapes, extract-validate-repair, tool call
L1  Abilities       atomic capabilities with cost and failure modes
                    ── understand, recall, judge, bind, invoke, express
L0  Substrate ops   combinators + pure ops + effects
                    ── Seq, Loop, Add, Strip, GetPath, Park
```

## 4.1 The uniform contract

```
Ability {
  id, version
  in:  TypedSchema
  out: TypedSchema
  fail: [ReasonCode]

  substrate:   Pure | Semantic | Llm | Effect
  cost_class:  Free | Cheap | Moderate | Expensive
  idempotent:  Bool
  effectful:   Bool
  suspendable: Bool

  preconditions:  [Predicate]
  postconditions: [Invariant]

  situation_key: Option<Vector>
  tool_deps:     [ToolId]
  prompt_hash:   Option<Hash>
  sensitivity:   none | pii | secret
}
```

**The whole trick is that a caller cannot tell what rung it is calling.** `Add` and `login` present the same interface. That is why:

- a flow step can be satisfied by a learned procedure without the flow being rewritten,
- `checkout` can compose `login` without knowing that `login` is a hundred primitives deep,
- and a composed block, once promoted, becomes indistinguishable from a primitive to the layer above.

That last point is the mechanism by which the system grows. Blocks become bigger blocks without a human authoring the composition.

## 4.2 Three fields that carry disproportionate weight

**`postconditions`** — this is what makes learned procedures safe inside authored flows. A flow step declares *what must be true when it completes*, never *how*. Any block or promoted procedure that satisfies the postcondition is admissible. Without this field, "learned procedures can satisfy flow steps" is unsound and you get silent violations of a flow's intent.

**`situation_key`** — the retrieval handle. The vector answers *have I been somewhere like here before*; the graph answers *and what did I do*.

**`tool_deps`** — the invalidation cascade. When a tenant redeploys and a tool's shape changes, every ability transitively depending on it is demoted automatically. Without this, learned procedures become landmines: confidently retrieved, silently broken.

---

# Part V — What the tenant declares

Five kinds of declaration. This is the entire surface of the product from the tenant's side, and everything above depends on it being rich enough.

## 5.1 Tools

```
ToolSpec {
  id, name, version
  capability_tags: ["auth.otp.send"]     ← how the system FINDS this tool
  effectful: true
  idempotent: false
  dry_run_available: true

  params: [ParamSpec]
  output_semantics: OutputSpec
  continuations: ["auth.otp.verify"]     ← what is expected to happen next
  errors: [ErrorSpec]
}
```

```
ParamSpec {
  name, type, required
  constraint:  range | pattern | enum | length | format(E164)
  source:      user | slot | state | env | derived(expr)
             | tool_output(ref) | const
  repair:      NormalizePhone
  prompt_hint: "What number should I send the code to?"
  sensitivity: pii
  default
  depends_on:  [field]
}
```

**`source` is the field that makes the difference between a system that feels intelligent and one that feels stupid.** Most tool arguments should never be asked for. The tenant ID is context. The timestamp is `env`. The phone number may already be in state from an earlier turn. Without `source`, the binder asks the user for things the system already knows — which is the single most common way agentic products annoy people.

**`capability_tags` is what keeps tool selection out of judgment.** When a flow step says "perform login here," the system does `Registry.LookupTool(tenant, capability: auth.otp.send)`. It **resolves**. It does not decide. No model involved.

**`continuations` is what makes the OTP flow work without inference.** In the login trace below, "the system understands an OTP will be sent" is not an inference at all — it's a field the tenant declared, confirmed by a matching response signature.

## 5.2 Personalities

```
PersonalitySpec {
  voice: { register, verbosity, formality, emoji_policy }
  lexicon: { preferred, forbidden }
  constraints: [ "never promise a delivery date",
                 "never quote a price without the pricing tool" ]
  templates: Map<TemplateId, Str>
}
```

**Hard rule: personality constrains `Express.*` and nothing else.** It never enters retrieval scoring, never enters policy evaluation, never enters the situation key. If it did, a tenant changing their tone would silently invalidate every procedure the system had learned.

## 5.3 States

Not passive labels. Each state carries two things:

```
StateSpec {
  permission_envelope: [Capability]        ← what is reachable FROM here
  direction: { target: StateRef, nudge_policy }   ← where we want them to go
  entry_conditions, exit_edges, timeout
}
```

The `direction` half is what lets the system nudge rather than merely react — the difference between an assistant and a product surface that drives outcomes.

## 5.4 Policies

```
PolicySpec {
  effect: Allow | Deny
  subject: { role, state, tenant, segment }
  action:  { capability, tool_id, transition, flow_id }
  condition: Predicate      ← CLOSED grammar
  reason_code, priority
}
```

The predicate grammar is deliberately closed. Predicates may reference only `state`, `role`, `slot`, `env`, `tool`, `consent`, `budget`, `evidence`, `flow_context`, combined with comparison, membership, range, and boolean operators. **No function calls, no loops, no I/O.** Closing the grammar is what stops policy from becoming tenant-supplied code that our server executes.

Policy applies at **three** points, not one: tool invocation, state transition, and flow activation.

## 5.5 Flows

```
FlowSpec {
  activation: {
    hard_preconditions: [Predicate]    ← evaluated FIRST, deterministic
    trigger_surface:    [Utterance]    ← embedding, breaks ties ONLY
    margin_threshold
  }
  learnable: false                     ← auth / payment / destructive
  preemption: Hold | SuspendYield | Yield
  steps: [FlowStep]
  escape: Fallback(flow) | FreeRange | Escalate
}

FlowStep {
  intent
  postcondition: Invariant     ← WHAT must be true, never HOW
  admissible: [AbilityRef | Capability]
  on_violation: Repair | Escape
  suspendable: Bool
}
```

Flows are **rails**. Once activated, the ordering is not negotiable and learning is structurally barred from proposing reorderings on `learnable: false` flows. That's what lets a tenant point at `flows.signup` and know exactly what will happen, every time.

But the *implementation* of each step stays open. Learning improves flows from underneath while the sequence stays frozen. **A flow is a contract about sequence, not about implementation.**

---

# Part VI — Anatomy of a turn

Now the actual machine. This is the hot loop, in order.

```
┌─ 0. SENSE ──────────────────────────────────────────────┐
│  Sense.Env      now, tz, locale, channel, turn_index    │
│  Sense.Session  open_loops, active_flow, pending_step   │
│  Sense.Budget   tokens/ms/calls remaining               │
│  State.Read     lifecycle position                      │
│  ── all pure, all free, all READ (never mutate) ──      │
└──────────────────────────┬──────────────────────────────┘
                           ▼
┌─ 1. FLOW GATE ──────────────────────────────────────────┐
│  pending_step present?  → ResumeFlow / AdvanceFlow      │
│  ── deterministic. no model. no embedding. ──           │
└──────────────────────────┬──────────────────────────────┘
                           ▼
┌─ 2. CLAUSE SPLIT ───────────────────────────────────────┐
│  pre-gate: word count, clause markers, length           │
│    → trivially single-clause? skip entirely (free)      │
│    → otherwise Understand.SplitClauses (LLM)            │
└──────────────────────────┬──────────────────────────────┘
                           ▼
┌─ 3. TRIAGE (per clause) ────────────────────────────────┐
│  Understand.ClassifyDepth   (semantic first)            │
│  Judge.Confidence → escalate if margin thin             │
│    shallow  → template path,  0 LLM                     │
│    boundary → single synthesis, no tools                │
│    deep     → continue                                  │
└──────────────────────────┬──────────────────────────────┘
                           ▼
┌─ 4. SITUATION KEY ──────────────────────────────────────┐
│  σ = ⟨ state, intent class, slots filled,               │
│        reachable capabilities, flow context,            │
│        bucketed turn_index, bucketed last_seen ⟩        │
└──────────────────────────┬──────────────────────────────┘
                           ▼
┌─ 5. TIER LOOKUP ────────────────────────────────────────┐
│  tier 0  exact σ hash hit        → run promoted path    │
│  tier 1  near hit, margin clears → run, rebind          │
│  tier 2  compose by type-search  → run, ledger as new   │
│  tier 3  LLM proposes path       → typecheck → supervise│
└──────────────────────────┬──────────────────────────────┘
                           ▼
┌─ 6. EXECUTE ────────────────────────────────────────────┐
│  policy invariant wraps every effectful step            │
│  postcondition checked after every step                 │
│  suspend on Park, resume later                          │
└──────────────────────────┬──────────────────────────────┘
                           ▼
┌─ 7. SYNTHESIZE ─────────────────────────────────────────┐
│  Judge.Sufficient → Express.Template | Express.Synthesize│
│  Judge.Groundedness on the synthesized output           │
└──────────────────────────┬──────────────────────────────┘
                           ▼
┌─ 8. WRITE BACK ─────────────────────────────────────────┐
│  turn record, facts, state transition, ledger, traces   │
│  ── saga, not transaction. idempotent, replayable. ──   │
└─────────────────────────────────────────────────────────┘
                           ║
                           ║ async, off the latency path
                           ▼
                    [ THE COLD LOOP ]
```

## 6.1 Two ordering rules that are easy to get wrong

**Sense before classify.** You cannot classify an utterance without knowing whether the system is mid-sentence with this person. The same three characters mean different things depending on session state.

**Flow context outranks utterance semantics.** If a `pending_step` exists, that wins — before triage, before embeddings, before anything. Getting this backwards is the commonest way these systems feel amnesiac.

---

# Part VII — Worked example 1: someone types "Hi"

This is the cheapest possible turn, which makes it the strictest test of the architecture. **If "Hi" costs an LLM call, the whole design is decoration.**

## 7.1 The first "Hi" this tenant has ever seen (cold path)

**Step 0 — Sense.**

```
Sense.Env      → { now: 2026-07-21T09:14Z, tz: Asia/Kolkata,
                   channel: web_widget, turn_index: 0 }
Sense.Session  → { open_loops: [], active_flow: null,
                   pending_step: null, last_seen: null }
State.Read     → StateNode{ id: "unauthenticated" }
```

Cost: three pure reads. Microseconds.

**Step 1 — Flow gate.** `pending_step` is null. Continue. One boolean check.

**Step 2 — Clause split pre-gate.**

```
SkipSplit = Or[
  WordCount("Hi") <= 3,                       → true
  Not(Contains("Hi", clause_markers)),        → true
  Length("Hi") < N_chars                      → true
]
```

All three trip. **The LLM-backed `SplitClauses` never runs.** This pre-gate is essential: `SplitClauses` sits at the top of every turn, so its cost is paid unconditionally, on every message, forever. A naive turn spine would burn an LLM call on "Hi" before it ever learned the turn was trivial.

**Step 3 — Triage.**

```
Understand.ClassifyDepth("Hi")   [semantic]
  → shallow,  margin 0.71
Judge.Confidence → margin > θ, no escalation
Understand.ClassifyReplyType     → Generic
```

Two embedding lookups. Sub-millisecond.

**Step 4 — Situation key.**

```
σ = ⟨ state:              unauthenticated
      intent:             greeting
      slots:              {}
      reachable_caps:     [auth.otp.send, catalog.browse]
      flow_ctx:           none
      turn_index_bucket:  first
      last_seen_bucket:   never ⟩
```

Note the **bucketing**. `turn_index` is `first | early | established`, not `0`. `last_seen` is `never | recent | lapsed | dormant`, not a timestamp. This matters enormously: raw values would give every greeting a unique σ, and tier 0 would have a 0% hit rate. You'd have built a cache that never hits.

Note also what is **absent**: personality. If personality entered σ, changing the tenant's tone would invalidate every learned procedure they had.

**Step 5 — Tier lookup.** No match. `Learn.LookupTier` returns tier 3.

**Step 6 — Cold path.**

```
Learn.ProposePath{σ, ability_set}     [LLM — 1 call]
```

The model is **not** asked to write a greeting. It's asked to select an ordered path over declared abilities. It returns:

```
Seq<
  State.Direction,          // is there a target state to nudge toward?
  Registry.Capabilities,    // what can this user actually do from here?
  Express.Synthesize
>
```

```
Learn.TypeCheck(path) → Ok
```

Every step's postcondition unifies with the next step's precondition. Had the model proposed something unsatisfiable — say, a tool call requiring an authenticated session — it would be rejected **structurally, before execution**, not discovered at runtime.

**Step 7 — Execute.**

```
State.Direction        → target: "authenticated"
Registry.Capabilities  → [browse_catalog, sign_in]
Express.Synthesize     → "Hey! I can help you browse the catalogue,
                          or sign you in to see your orders."
```

**Step 8 — Write back.**

```
Learn.Propose{ σ, path, outcome } → proposal_id
Remember.WriteTurn
LedgerAppend
```

One detail worth naming: **the greeting opens no loop.** A greeting with no request is not an unresolved obligation. If you open loops on greetings, the proactive daemon starts nudging people who said hello and left — precisely the behavior that makes a tenant's customers hate the product.

**Total: 1 LLM call.** This happens once.

## 7.2 The same "Hi", after promotion (warm path)

Steps 0–4 are identical. At step 5:

```
Learn.LookupTier{σ} → tier 0, exact hash hit
```

**Step 6 — Execute the promoted path.**

```
State.Direction        [pure]     → target: authenticated
Registry.Capabilities  [pure]     → [browse_catalog, sign_in]
Express.Template{greeting_unauth, bindings}   [pure]
```

The greeting resolved to a **template** rather than synthesis because `ClassifyReplyType` returned `Generic`, and generic replies dispatch to `Express.Template`. That's free, auditable, and legally reviewable by the tenant — which matters when your customer is a regulated business.

The template is not static. It's filled from the direction lookup and the capability list, so the response differs by situation:

| Situation | Same op, different bindings |
|---|---|
| unauthenticated, first turn | "Hey! I can help you browse, or sign you in." |
| authenticated, returning today | "Hi again — want to pick up where you left off?" |
| authenticated, dormant 3 weeks | "Welcome back! It's been a while — here's what's new on your account." |

**Total: 0 LLM calls. ~2–4ms.**

## 7.3 The same "Hi", from a user parked mid-flow

```
Sense.Session → { active_flow: "login",
                  pending_step: "await_otp",
                  parked_at: 09:02Z }
```

Step 1, the flow gate, fires. **Triage never runs.** The utterance's semantic content is irrelevant.

```
ResumeFlow
  → re-check policy      [state may have changed while parked]
  → re-check pinned versions
  → AdvanceFlow → still awaiting otp
  → Express.Template{otp_reminder}
     "Welcome back — I still need the 6-digit code we sent to
      ••••••3421. Want me to resend it?"
```

Same three characters. Completely different turn. **This is why flow context outranks utterance semantics.**

---

# Part VIII — Worked example 2: login with OTP

The trace that shows tool resolution, declared continuations, suspension, and why some things must never be learned.

## 8.1 The full path

```
STATE: unauthenticated
   │
   ├─ ACTIVATION GATE                              [deterministic]
   │    hard precondition: state == unauthenticated       ✓
   │    Policy.Evaluate(activate, flows.login)            ✓ Allow
   │    situation match vs trigger_surface, margin 0.83   ✓
   │    ── embeddings only broke the tie. Preconditions filtered first.
   ▼
FLOW: login    [learnable: false]
   │
   ├─ STEP 1  "what performs login here?"
   │    Registry.LookupTool(tenant, capability: "auth.otp.*")
   │      → ToolSpec{ send_otp, verify_otp }
   │    ── RESOLVED from declared capability tags.
   │       No model. No guessing. This is why tags matter.
   │
   ├─ STEP 2  collect phone
   │    Bind.ResolveAll(send_otp, sources)
   │      → tenant_id  ← const        (never asked)
   │      → timestamp  ← env          (never asked)
   │      → phone      ← NeedsUser    (the only residual)
   │
   │    Express.Ask{phone, hint} → "What number should I text the code to?"
   │    ══════════ TURN BOUNDARY ══════════
   │    user: "98765 43210"
   │
   │    Understand.Extract(text, PhoneSchema)   [semantic first]
   │    Judge.Confidence → margin 0.91, no escalation
   │    Bind.Normalize(NormalizePhone, region: IN) → "+919876543210"
   │    Bind.Validate(format: E164) → Ok
   │    ── repair loop, max 3 attempts, on Violation
   │
   ├─ STEP 3  send
   │    Policy.Evaluate(invoke, send_otp)     ← INVARIANT, not composed
   │    IdemKey(user, send_otp, phone, window)
   │    Tee → LedgerAppend{ intent, pending }
   │    Once → Invoke.Call(send_otp, {phone}, idem)
   │    Tee → LedgerAppend{ receipt, success }
   │
   │    Sig.Compute(raw) → Sig.Hash → Sig.Match
   │    Sig.Classify → EffectConfirmation + Continuation
   │      ── the ToolSpec DECLARED continuations: ["auth.otp.verify"]
   │      ── "the system understands an OTP will be sent" was never
   │         an inference. It read it off the spec.
   │
   ├─ STEP 4  ══════ SUSPEND ══════
   │    persist: slots, pending_step, ttl=300s, attempts=0,
   │             pinned versions (flow, tool, prompt hashes)
   │    Park{until: Event(user_message) | Ttl(300s)}
   │    ── NOT Sleep. A blocking sleep would hold a worker
   │       for the entire OTP window.
   │    ══════ TURN ENDS. Could be 10s or 2 hours. ══════
   │
   ├─ STEP 5  resume
   │    ResumeFlow
   │      → re-check Policy.Evaluate      ← may have changed while parked
   │      → re-check State.Read           ← may have changed while parked
   │      → check pinned versions still valid
   │           invalid? → flow's declared escape fires.
   │                      It does NOT silently run against new semantics.
   │
   │    user: "434543"
   │    Understand.Extract(text, OtpSchema) → "434543"
   │    Invoke.Call(verify_otp, {phone, otp}, idem)
   │
   └─ STEP 6  transition
        State.ProposeTransition(user, "authenticated", evidence: receipt)
        LedgerAppend
        Express.Synthesize → "You're in. What would you like to do?"
```

## 8.2 Three things this trace demonstrates

**Tool resolution is a lookup, not a decision.** Step 1 works because the tenant declared `capability_tags`. If they hadn't, step 1 becomes a judgment call, and the whole design leaks at exactly the point where it must not. This is why Part V is the keystone.

**Suspension is harder than sequencing.** Steps 1–3 are one turn. Step 4 might be two hours. The instance has to persist with slots, pending step, TTL, attempt counter, and pinned versions — and on resume it must re-verify policy and state, because both may have changed. **A resumed flow that trusts its pre-suspension authorization is a security hole.**

**Some things must never be learned.** Login is `learnable: false`. Nobody should discover from evidence that you check for an existing account first, and the learning system must be *structurally barred* from proposing reorderings — not merely disinclined. Auth, payment, and deletion all live in this class.

What learning *is* still allowed to touch underneath a frozen sequence: the extraction step's escalation threshold, the repair prompt's wording, the retry timing. Implementation, never order.

---

# Part IX — Worked example 3: "give me the ten most avaricious clients"

This is the example that exposes where naive semantic search fails dangerously.

## 9.1 The trap

The tenant's data has an attribute called `discount_pressure_score`. Nobody has ever used the word "avaricious." The obvious approach — embed "avaricious," find the nearest stored concept — has a specific and severe failure mode:

> **Antonyms are distributionally close.**

"Greedy" and "generous" appear in nearly identical linguistic contexts. Their embeddings sit near each other. A pure nearest-neighbour match on "avaricious" can plausibly retrieve your *least* demanding clients and return them with complete confidence.

In a CRM, that's a report the customer acts on. It is wrong, it is confident, and nothing in a similarity score will tell you.

## 9.2 How it actually works

Term resolution is **anchored to declared attributes**, never free-floating over text.

```
Attribute "discount_pressure_score" {
  anchors:  ["greedy", "haggler", "price-sensitive",
             "always negotiating"]        ← tenant-declared
  learned:  ["tight-fisted"]              ← promoted after confirmations
  polarity: high = more pressure          ← THE CRITICAL FIELD
  sortable: true
}
```

The trace:

```
"give me the ten most avaricious clients"
   │
   ├─ Pre-gate: 6 words, no clause markers → single clause
   ├─ ClassifyDepth → deep
   │
   ├─ TERM RESOLUTION
   │    Recall.Embed("avaricious")
   │    Recall.Semantic over attribute anchor space
   │      → discount_pressure_score   0.79
   │      → generosity_index          0.74     ← the antonym, RIGHT THERE
   │    Judge.Confidence → margin 0.05  ← THIN
   │
   ├─ MARGIN TOO THIN → CONFIRM ONCE
   │    Express.Clarify
   │      "Reading 'avaricious' as discount-pressure score —
   │       clients who push hardest on price. Right?"
   │    user: "yes"
   │
   ├─ WRITE THE SYNONYM
   │    Remember.LinkEntity("avaricious" → discount_pressure_score,
   │                        edge: synonym_of, confirmations: 1)
   │    ── after N confirmations it is promoted and free forever
   │
   ├─ QUERY PLAN FRAGMENT
   │    { attribute: discount_pressure_score,
   │      direction: desc,          ← FROM POLARITY, not from embedding
   │      limit:     10 }
   │    ── "most" + polarity(high = more) → desc
   │       Without polarity, "most" and "least" are the same
   │       embedding neighbourhood.
   │
   ├─ TOOL RESOLUTION
   │    Registry.LookupTool(capability: "crm.clients.query")
   │    Bind.ResolveAll → { tenant ← const,
   │                        sort_by ← derived(attribute),
   │                        order   ← derived(direction),
   │                        limit   ← derived(limit) }
   │    Policy.Evaluate(invoke, clients.query) → Allow
   │    Invoke.Call → 10 rows
   │
   ├─ SANITATION
   │    Sig.Compute(raw)          ← BEFORE any cleaning
   │    Sig.Match → promoted plan
   │    Invoke.Extract
   │    Clean → Redact(pii)  ← emails/phones stripped before prompting
   │    Enrich(provenance: tool, call_id, receipt)
   │
   └─ SYNTHESIS
        Express.Synthesize(evidence, personality, constraints)
        Judge.Groundedness → 0.94, no unsupported spans
```

## 9.3 Why the confirmation is worth it

It costs one extra turn, **once**, in the lifetime of that tenant's vocabulary. After promotion, "avaricious" resolves for free forever, for every user of that tenant. And it converts a silent, confident, wrong answer into a thirty-second clarification.

The general principle: **thin margin plus consequential action equals confirm.** That's not timidity, it's the only correct behavior when the two nearest candidates are opposites.

## 9.4 The second trap in the same sentence

"Ten most" requires a **rankable** field. Semantic matching that resolves only to a topical filter can't answer it — it can find *related* clients but not *ordered* ones. That's why term resolution outputs a **query-plan fragment** `{attribute, direction, limit}` rather than a similarity score. It has to resolve to something sortable, or the query is unanswerable regardless of how good the match was.

---

# Part X — Worked example 4: the vocabulary composing itself

This is the one that shows why the system is more than a cache.

Situation: a user asks something the system has genuinely never seen, and **no LLM call is made anyway.**

## 10.1 The setup

The tenant's vocabulary, after a week of `bake` and traffic:

```
promoted procedures:
  P1  resolve_customer_by_name    σ: has person name, needs customer id
        Seq< Understand.Extract(NameSchema),
             Recall.Lexical(customers),
             Recall.Rerank,
             Judge.Confidence >
        post: customer_id resolved

  P2  fetch_open_invoices          σ: has customer id, wants invoices
        Seq< Bind.ResolveAll(invoices.list),
             Invoke.Call,
             Sig.Match, Invoke.Extract,
             Filter(status = open) >
        pre:  customer_id present
        post: invoice list present
```

## 10.2 The novel request

```
"what does Acme still owe us?"
```

No procedure matches σ. Tier 0 misses. Tier 1 misses.

**Tier 2 — type-directed compositional search.**

```
goal:      answer requires  invoice_list
available: P2 produces      invoice_list
           P2 requires      customer_id
           P1 produces      customer_id
           P1 requires      person_or_org_name  ← present in utterance

path found:  P1 → P2 → Sum(amount_due)

Learn.TypeCheck:
   P1.post  ⊨  P2.pre      ✓
   P2.post  ⊨  Sum.pre     ✓
   all slots sourced        ✓
   Policy.Evaluate on each  ✓
   loops bounded            ✓ (none)
```

Execute. Answer produced. **Zero LLM calls except terminal synthesis.**

## 10.3 Why this is the whole point

Without tier 2, every novel phrasing costs an LLM proposal, forever. The vocabulary would *accumulate* but never *compose* — a cache with a very large key space.

With tier 2, a system that knows two things handles a third it was never taught. That's the difference between memorization and vocabulary. And the composed path is itself ledgered as a new candidate procedure, so if "what does X still owe us" turns out to be common, it promotes to a single tier-0 hit and gets even cheaper.

---

# Part XI — The cold loop: how the system learns

Everything so far was the hot loop. Now the part that runs asynchronously, off the latency path.

```
turn completes
     │
     ├─ SCORE            Judge.Outcome ← real signals, never self-grade
     │     tool returned non-error?
     │     did the user's next turn REPAIR? ("no, I meant—")
     │     did the flow reach terminal state?
     │     did the user abandon mid-sequence?
     │     latency, token cost
     │
     ├─ ATTRIBUTE        Learn.ScoreStep per step, then Learn.Attribute
     │     ── PER-STEP, not per-path.
     │        A six-step failure demotes the failing step,
     │        not the five good ones around it.
     │
     ├─ PROPOSE          Learn.Propose → harness_proposals
     │     ── evidence accumulates here, unpromoted
     │
     ├─ PROMOTE          Learn.Promote, gated
     │     ≥ N clean observations
     │     success rate above threshold
     │     cost within budget
     │     effectful? → TENANT APPROVAL REQUIRED
     │
     ├─ INVALIDATE       on tool change OR prompt-hash change
     │     cascade over tool_deps and prompt dependents
     │     → demote everything transitively affected
     │
     └─ EXPLORE          ε fraction of tier-0 hits run the runner-up
           ── records comparative outcome
```

## 11.1 The success signal must have teeth

An LLM asked "was that good?" says yes far more often than it should. It grades its own homework and it's a soft grader. So the signal is behavioural:

| Signal | Meaning |
|---|---|
| tool non-error | weak positive |
| flow reached terminal state | strong positive |
| **user repair turn** ("no, I meant...") | **strong negative — the cheapest honest one you have** |
| abandonment mid-sequence | strong negative |
| latency / cost | efficiency, not correctness |

`Understand.DetectRepair` therefore earns its place as a first-class ability. It is the highest-value negative signal in the system, and it's free — it comes from the user's very next message.

## 11.2 Why exploration is not optional

If you only promote paths that worked, you never discover the path that would have worked *better*, because you stopped sampling it. Classic exploit-without-explore.

Concretely: whatever the LLM proposes on the very first cold-path encounter becomes muscle memory, and tier 0 then hits forever. **Every procedure freezes at "the first thing that worked,"** which is usually not "the thing that works well." The system stops being a learning organism about three weeks after launch.

So: an ε fraction of tier-0 hits deliberately run the runner-up candidate and record the comparison. That's what gives the exploration budget somewhere to put its findings, and it's why promoted procedures are **immutable with v2 created alongside v1** — you need both retrievable, ranked by evidence, so a "better" path that loses under real traffic can be rolled back.

## 11.3 Validating a proposal without breaking things

You cannot validate a proposed procedure by running it when it contains `create_account` or `charge_card`. Three tiers:

```
TIER 1  STATIC          free, instant, kills most bad proposals
        postcondition ⊨ precondition through the whole chain
        every slot sourced
        policy admits every step
        every loop bounded

TIER 2  LEDGER REPLAY   ← the strongest asset, and it already exists
        run the proposal against RECORDED tool responses
        no side effects, real data shapes, real edge cases
        ── the ledger is a free regression corpus.
           Build this before anything else in the learning loop.

TIER 3  LIVE SHADOW     read-only paths only
        run alongside the promoted path, serve promoted,
        log every disagreement

EFFECTFUL STEPS         never validated by execution. Ever.
        pass on type + policy + tenant approval,
        enter production behind the exploration budget with a hard cap
```

This creates a genuine product incentive worth documenting in the SDK: **tenants who expose a dry-run mode get faster procedure learning on their write paths.**

---

# Part XII — The tool call in depth

The return path deserves its own treatment, because this is where silent corruption enters a system like this.

## 12.1 Three separate things people conflate

```
1. RESPONSE SIGNATURE   structural fingerprint, learned, cheap
   { key_set, depth, type_per_path, cardinality,
     value_features: { length: 6, char_class: digits,
                       pattern: ^\d{6}$ } }
   → hashed. THIS is the retrieval key.

2. EXTRACTION PLAN      bound to a signature hash, promoted
   otp ← $.otp :: String(6, numeric)
   → once promoted, applying it is a PURE operation.
     Zero model. Microseconds.

3. SEMANTICS            what the value MEANS
   → this is DECLARED at registration, never learned.
```

**Learn the structure. Declare the meaning.** Structure can tell you `$.otp` is six digits. It cannot tell you the value is a secret, expires in 300 seconds, must be echoed to `verify_otp`, and must never reach a log or a prompt.

If you try to infer meaning, `{"code": "434543"}` is indistinguishable from a discount code — and you will leak an OTP into a ledger.

## 12.2 The lifecycle

```
call 1..N     signature unknown / plan unpromoted
              → Invoke.Interpret (LLM)
              → validate against declared output semantics
              → Sig.Propose → ledger

call N+1..    signature hash matches promoted plan
              → apply plan (pure)
              → verify postconditions
              → NO LLM. microseconds.

any call      hash does NOT match
              → DO NOT GUESS
              → fall back to Invoke.Interpret
              → open a new proposal branch
```

That mismatch branch is the entire safety story. **A cached extractor applied to a response it wasn't derived from is silent corruption — worse than an error, because it succeeds.** The signature check runs before extraction, always.

It's also your tool-side invalidation cascade, and it needs no notification: the tenant changes their API, the hash stops matching, the plan stops being used. Automatically.

## 12.3 Errors are signatures too, and worth more

```
{"error": "rate_limited", "retry_after": 30}
   → ReasonCode: RateLimited
   → recovery:   Retryable(after: 30s)

{"error": "invalid_otp", "attempts_left": 2}
   → ReasonCode: NeedsRepair
   → recovery:   re-ask, decrement attempts

{"error": "account_locked"}
   → ReasonCode: Terminal
   → recovery:   NeedsEscalation
```

Mapping errors to a closed reason-code taxonomy with a declared recovery is exactly as learnable as the success path, and it's what turns the executor from brittle into resilient. It should be designed in from the start, not bolted on when production starts failing.

## 12.4 Response roles

Four, and they drive different executor behavior:

| Role | Executor does |
|---|---|
| `data` | bind payload into slots |
| `effect_confirmation` | record the receipt; body may not matter |
| `continuation` | enqueue the declared expected next call |
| `error` | classify → recovery |

`continuation` is what made the login trace work without any inference at all.

---

# Part XIII — Where it all lives

## 13.1 The storage principle

```
Sunjet stores the durable truth, the fast search surfaces,
the graph continuity, and the audit trail.

The harness decides what to write, what to retrieve,
what to score, and what enters the prompt.

The LLM only reasons over the already-selected contract.
```

## 13.2 Isolation

**Per-tenant table names.** Sunjet's isolation unit is the table, so the tenant boundary becomes *structural* — you cannot leak by forgetting a filter, because the table name **is** the filter.

The cost: schema migration is now an N×26 operation, which is why a migration runner is required from day one. System-owned tables (the operation registry, prompt specs, system docs) stay shared and read-only to avoid N-way duplication. All of it resolves through one function, `resolveTable(tenant, logical_name)`, so the decision stays reversible.

## 13.3 The write path is a saga, not a transaction

Sunjet exposes row-level mutations, no multi-row transaction. So a turn's writes are ordered, idempotent, and replayable:

```
1. deterministic turn_id and message_id
2. append user message
3. trace: turn_started
4. context/memory/evidence rows, all tagged with turn_id
5. BEFORE any side effect: ledger intent (pending)
6. execute tool
7. ledger success/error
8. state transition, with source tool/turn id
9. assistant reply
10. trace: turn_completed
11. reconciler repairs partial turns
```

Every write is idempotent by logical key or content hash, linked to `turn_id`, safe to replay, and auditable if partially complete.

## 13.4 Conditional write unblocks four races at once

The one Sunjet feature that matters most is not transactions — it's compare-and-set:

```
insertRowIf(table, row, predicate)             // insert-if-absent
updateRowIf(row_id, expected_version, patch)   // optimistic concurrency
```

That single primitive fixes: `Once{idem_key}` deduplication, double-promotion by concurrent sweepers, multi-process job leasing for parked flows, and one-active-suspension-per-session. Until it lands, single-writer-per-tenant gives serialization without DB support — and per-tenant tables already supply the shard key.

## 13.5 New tables the learning layer needs

| Table | Why |
|---|---|
| `harness_procedures` | L3 learned compositions. **Operations are declared; procedures are earned** — different lifecycle, different table. |
| `harness_signatures` | response shape → hash → extraction plan → promotion state |
| `harness_proposals` | where evidence accumulates before promotion fires |
| `harness_prompts` | versioned PromptSpec; `prompt_hash` referenced by operations |
| `harness_docs` + `harness_doc_chunks` | the virtual document store |

---

# Part XIV — Prompts are dependencies, not strings

A subtle point with large consequences.

```
PromptSpec {
  id, version, ability_ref
  frame                        // task + closed output vocabulary
  slots: [{ name, source, max_tokens, required, redact }]
  output_contract: Schema      // what the parser will enforce
  examples
  budget: { max_tokens, truncation_order }
  hash
}
```

Assembly is deterministic: resolve slots by source, fill in fixed order, truncate by **declared priority** when over budget, emit. Same inputs → byte-identical prompt. That's what makes replay meaningful.

**`prompt_hash` is part of the ability's version.** Change a prompt and you have changed the ability's behavior — which stales every calibrated escalation threshold that depended on it, and makes every learned procedure that depended on that behavior suspect. **Prompt changes must cascade invalidation exactly like tool changes.** This is the invalidation path nobody builds until it bites them.

Truncation order must also be declared rather than implicit. If it isn't, the same situation produces different prompts under different context loads, and you have silent nondeterminism in the one place you were relying on determinism.

---

# Part XV — Safety, failure, and the parts that keep it sellable

## 15.1 Policy is an invariant, not a step

`Policy.Evaluate` is never a model, and it is **not composed into paths** — it wraps every `Invoke.Call` and every `State.ProposeTransition` structurally. The composer cannot forget it because the composer never sees it.

## 15.2 Degradation

A conversational interface that hangs is worse than one that admits it can't reach the system.

```
Fallback<
  full capability,
  reduced (cached / read-only),
  template acknowledgement,
  human handoff
>
```

Every layer declares an escape. Flows declare theirs. Budget exhaustion is a typed outcome (`BudgetExceeded`) that triggers degradation, not a crash.

## 15.3 Sensitive data

`sensitivity: none | pii | secret` on every parameter and every extracted field. Redaction runs **before** anything enters an LLM context window, and before it enters the ledger. The OTP never reaches a prompt.

## 15.4 Ambiguity is a legitimate outcome

`Understand.ResolveReference` returns `Ambiguous[..]`, never a best guess. In a business context, confident-wrong resolution is the worst possible failure — worse than asking. Ambiguity is a typed outcome that triggers a clarification block.

---

# Part XVI — The tenant lifecycle

This is what makes the system usable on day one rather than embarrassing.

```
┌── BAKE ──────────────────────────────────────────────┐
│  onboarding, no live traffic                          │
│  tier-3 heavy — the LLM proposes a lot                │
│  sandbox: static check + ledger replay                │
│  tenant approves effectful procedures                 │
│  LLM budget high, latency irrelevant                  │
│  ── the vocabulary is built here                      │
└──────────────────────┬───────────────────────────────┘
                       ▼
┌── LIVE ──────────────────────────────────────────────┐
│  production traffic                                   │
│  tier-0/1 is the target; tier-3 is RARE and ALARMABLE │
│  small exploration budget ε                           │
│  effectful promotions still tenant-gated              │
│  p95 ≤ 400ms warm, ≤ 1.5s with one tool               │
└──────────────────────┬───────────────────────────────┘
                       ▼
┌── REBAKE ────────────────────────────────────────────┐
│  triggered by tool change or prompt-hash change       │
│  affected procedures demoted, partial re-bake         │
└──────────────────────────────────────────────────────┘
```

**No tenant ever meets a real user with an empty vocabulary.** That single decision removes the cold-start problem that would otherwise make first impressions slow, expensive, and erratic.

---

# Part XVII — How you know it works

The practical payoff of decomposing everything into typed functions is not elegance. It's **testability**. A system where the LLM decides things can only be evaluated end-to-end, by vibes. This one can be unit tested.

| Method | What it catches |
|---|---|
| **Golden traces** — ~30 hand-written expected step sequences, diffed against actual | shows exactly which step diverged. The single most valuable early artifact. |
| **LLM-call assertions** — `assert llm_calls("Hi", warm) == 0` as a hard test | stops model calls creeping back into hot paths. Without it you find out from the bill. |
| **Tier-0/1 hit rate** | the headline metric. Low means σ is over-specified — a cache that never hits. |
| **Replay** — deterministic steps must produce byte-identical results | catches non-determinism leaking where it shouldn't |
| **Shadow mode** — run cheap and expensive side by side, serve cheap, log disagreement | the only honest way to set escalation thresholds |
| **Fault injection** — garbage responses, timeouts, changed shapes | the dangerous failure isn't a crash, it's a stale extraction plan that succeeds with the wrong field |
| **Escalation rate per ability** | early warning that the semantic layer has drifted from tenant reality |

### The determinism budget

| Turn type | LLM calls |
|---|---|
| shallow | 0 |
| boundary | 1 |
| deep, tier 0/1 | 1 (terminal synthesis only) |
| deep, tier 2 | 1–2 |
| deep, tier 3 | 3–5 — should trend toward 0% of live traffic |

---

# Part XVIII — Why each piece exists

A compressed defence of the design.

| Piece | Without it |
|---|---|
| Typed abilities with contracts | composition is impossible; the composer can't reason about pieces it can't introspect |
| Determinism sandwich | an LLM call poisons everything downstream of it |
| Uniform contract at every rung | blocks can't grow into bigger blocks; every level needs bespoke plumbing |
| Postconditions on flow steps | learned procedures satisfying flow steps is unsound |
| Situation key with bucketing | tier 0 has a 0% hit rate; you built a cache that never hits |
| Tier 2 compositional search | it's a cache, not a vocabulary; every novel phrasing costs an LLM call forever |
| Response signatures | every tool response costs an interpretation call |
| Signature mismatch → never guess | silent corruption: wrong field, confidently extracted |
| `source` on parameters | the system asks users for things it already knows |
| `capability_tags` on tools | tool selection becomes a judgment call, and the design leaks |
| `continuations` on tools | multi-call sequences require inference |
| Declared semantics, learned structure | you leak an OTP into a log |
| Polarity on attributes | "most X" and "least X" retrieve the same clients |
| Repair detection | the learning loop has no honest negative signal |
| Per-step credit | one bad step poisons five good words in the vocabulary |
| Exploration budget | every procedure freezes at the first thing that worked |
| Immutable versions | no rollback when "better" turns out worse |
| `tool_deps` cascade | procedures become landmines: confidently retrieved, silently broken |
| `prompt_hash` in the version | thresholds and procedures silently stale after a prompt edit |
| Closed policy grammar | policy becomes tenant-supplied code your server executes |
| Policy as invariant | the composer forgets to check it exactly once, and that's the incident |
| Bounded loops | a learned procedure hangs a turn and drains a tenant's budget |
| `Park` not `Sleep` | a worker is held for the entire OTP window |
| Re-check on resume | a two-hour-parked flow acts on stale authorization |
| Version pinning on suspension | a resumed flow runs against redeployed semantics |
| Per-tenant tables | isolation depends on every query remembering a filter |
| Conditional write | four separate races, all silent, all intermittent |
| Bake mode | every tenant's first week is slow, expensive, and erratic |
| Golden traces | you cannot tell whether a change made the system better |

---

# Part XIX — What is still open

Carried forward. Not blocking, but each needs an answer during implementation.

1. **Memory taxonomy** — episodic / factual / entity / procedural / open-loop, with distinct write rules, decay, and retrieval shapes. Entity identity resolution across turns is the hard sub-problem.
2. **Evidence signal enumeration** — the exact set feeding `Judge.Outcome`.
3. **Nudge semantics** — the `direction` half of state is structurally specified, its policy unwritten.
4. **Cross-tenant generalization** — default no; per-tenant tables make it structurally hard, which is consistent. Still a genuine strategic question.
5. **Calibration harness** — every `⇄` pair needs empirical thresholds, per ability, per tenant.
6. **Multi-clause conflict** — resolution order when split clauses contradict each other.
7. **Exploration rate ε** — value, decay schedule, per-flow opt-out.
8. **Deployment topology** — SDK push vs pull, scheduler placement, horizontal scale given a single-node Sunjet.
9. **Whole-system failure** — the full degradation ladder when Sunjet, the model provider, or the tenant's API is down.
10. **Deletion** — a user's turns, facts, entities, and embeddings are deletable; what about a procedure learned partly from their behavior?

---

# Part XX — The shape of the thing, in one page

```
A tenant declares five things:
    tools, personalities, states, policies, flows.

Everything the system can do is a typed ability with a contract.
Abilities live in four substrates: pure, semantic, llm, effect.
LLM abilities are sandwiches: deterministic in, deterministic out.

Abilities compose into blocks.
Blocks compose into procedures — which are LEARNED, not written.
Procedures satisfy the steps of flows — which are AUTHORED, and frozen.
Flows run inside states, gated by policies.

Every rung exposes the same contract, so the layer above
cannot tell what it is calling. That is how small things
become large things without anyone writing the large thing.

Per turn, the hot loop is deterministic:
    sense → gate → triage → situation key → tier lookup
          → execute → synthesize → write back.

The LLM appears in exactly two places:
    inside a sandwich, and at the final sentence.
    Plus, on genuinely novel situations, as a proposer of paths
    that the type system then verifies.

After the turn, the cold loop runs off the latency path:
    score from real behaviour → attribute per step
          → propose → promote → invalidate → explore.

Over time, tier 3 shrinks, tier 0 grows,
the vocabulary composes itself at tier 2,
and the cost per conversation falls toward zero.

That is the organism.
```
