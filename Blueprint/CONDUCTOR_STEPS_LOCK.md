# Conductor steps — product lock (working)

**Opened:** 2026-08-05  
**Rule:** Build end-to-end from *these* steps. Keyword ladders are **not** Conductor.  
**Shape:** Conductor is a harness = inputs → ordered steps → output / park / error.

Update this file whenever Sanjith clarifies a step. Periodic notes go here + `CONDUCTOR_HARNESS_MONITORING_NOTES.md`.

---

## 0. What a harness is (locked)

- Ordered steps.
- Optional inputs, optional outputs (`unit` if none).
- Ends with: result | typed error | park (wait) | return to parent.

Conductor is the **ceiling harness** — same shape, special job: route the session.

---

## 1. Conductor inputs (proposed)

| Input | Meaning |
|-------|---------|
| `in.utterance` | User / system text for this event |
| `in.event_type` | e.g. `user.message`, webhook, push |
| `in.stack_summary` | What’s on the stack (ids, waiting?) |
| `in.page` | Conductor context page (notes/slots) |
| `in.catalog` | Installed harness ids + **descriptions** (from DB) |
| `in.waiting_child` | If set, child owns the turn unless exit/fresh |

---

## 2. Conductor outputs (proposed)

| Output | Meaning |
|--------|---------|
| `out.decision` | `reply` \| `spawn` \| `continue_child` \| `exit_up` \| `fresh` \| `ignore` |
| `out.harness_id` | When spawn — which program |
| `out.spawn_args` | Projected inputs for that child |
| `out.reply` | When reply — user-facing text (or handle from child) |
| `out.error` | Typed failure if any |

---

## 3. Conductor steps (DRAFT — confirm / edit)

These are the steps we want in the **stored Conductor program**, not Rust `if contains`.

### Step 0 — Stack gate (no LLM)

**If** a child is `waiting`:

- If utterance is clear **exit_up** / **fresh** → go to those steps.
- Else → **continue_child** (hand utterance to waiting harness; Conductor does not re-arbitrate).

This matches the vision: waiting frame owns the next message.

### Step 1 — Load context

- Read Conductor page.
- Load catalog: `harness.list` + `harness.describe` (installed programs only).
- Attach utterance + event type into the bag.

### Step 2 — Decide (the real Conductor brain)

- Call a **stored prompt** + `llm.classify` (or equivalent admitted Call) with:
  - utterance
  - page / recent context
  - catalog summaries
  - allowed decisions: `reply` | `spawn` | `exit_up` | `fresh` | `ignore`
- Model must return structured decision:
  - if `spawn` → must pick an **existing** `harness_id` from catalog (or refuse → `reply` / escalate policy)
  - if `reply` → enough to answer casually from context

**Not allowed as authority:** keyword lists for tool verbs / intent.

Optional **fast path in front of Step 2** (product choice): exact tiny greets (`hi`/`hello`) → `reply` with 0 LLM. Everything else goes through Step 2.

### Step 3 — Act on decision

| Decision | Steps |
|----------|--------|
| `reply` | Invoke reply harness (e.g. `quick_reply`) **or** inline `llm.generate` with Conductor reply prompt → finish |
| `spawn` | Build `spawn_args` → `harness.invoke` / `harness.spawn` child → wait for child output or park |
| `exit_up` | Pop one layer (no-op at ceiling / stay Conductor) |
| `fresh` | Clear stack → restart at Conductor with clean page policy |
| `ignore` | Ack/no-op for system events that need no user reply |
| `continue_child` | Resume waiting child with utterance (from Step 0) |

### Step 4 — After child returns

- Merge child `out.*` into Conductor page as needed.
- Either:
  - compose final user reply, or
  - loop decide again (another spawn / reply), or
  - park if still waiting on user/tool.

### Step 5 — Terminate turn

- Return reply to channel, **or**
- Park continuation (durable), **or**
- Typed error / safe stop.

---

## 4. What Conductor is *not*

- Not a Rust keyword router.
- Not “say Spawning X” without actually running X.
- Not inventing harness ids that aren’t installed (unless explicit draft/promote path — Phase 7).
- Not owning the turn while a child is waiting (except exit/fresh).

---

## 5. Open choices for Sanjith (answer to lock)

1. **Reply:** always spawn `quick_reply` harness, or Conductor may LLM-reply inline?
2. **Fast path:** keep 0-LLM exact greets, or everything through Step 2?
3. **No catalog fit:** casual `reply` only, or allow cold `Escalate` / ProposePath as explicit decision?
4. **Spawn:** synchronous `invoke` (wait for result) vs `spawn` (push stack, child may park)?
5. **System pushes:** same Conductor steps, or a thinner event Conductor?

---

## 7. Abstract Conductor (walk-through lock — 2026-08-05 evening)

**Owner framing:** think more generic. Conductor is a program of a few high-level moves.
Even “spawn” returns a **value** to Conductor, which then decides again.

### Conductor’s menu (what it can choose)

Not keyword routes — abstract actions:

| Action | Meaning |
|--------|---------|
| **quick_reply** | Short, cheap answer (light / template / small LLM) |
| **rough_chat** | Fuller LLM reply from context (casual talk) |
| **spin_harness** | Run another harness as a subroutine; get `value` back (or park) |
| **exit_up / fresh** | Leave layer / restart (when relevant) |

`spin_harness` is itself generic: *invoke named program with args → get result value*.
That result is just data Conductor uses on the next decide.

### Loop (the algorithm)

```text
on event:
  inputs → Conductor
  loop:
    decide ∈ { quick_reply, rough_chat, spin_harness, exit_up, fresh, stop }
    if quick_reply / rough_chat → produce text → stop (or soft continue)
    if spin_harness(id, args):
         child runs its steps
         returns value | error | park
         if park → wait; next user msg resumes child (stack gate)
         if value → Conductor page/bag gets it → decide again
    if exit_up / fresh → do stack op → maybe decide again
```

So: **decide → act → (maybe get value) → decide again** until reply or park.

### Example: `"i want jobs"`

1. Decide → `spin_harness(jobs.list, …)` **or** if none installed → `rough_chat` / `quick_reply`
2. If spun: child returns `{ jobs: [...] }` 
3. Conductor decides again → `quick_reply` / `rough_chat` to present them  
   (or spin another harness to filter/sort)

### Example: needs more info

1. Decide → `spin_harness(wait_for_user / clarify, …)`
2. Child parks (“what city?”)
3. User answers → child completes → value to Conductor
4. Conductor decides → maybe `spin_harness(jobs.list)` with city filled

### Abstraction rule

- Don’t special-case “OTP path” inside Conductor.
- Special behavior lives **inside** named harnesses.
- Conductor only: **talk** vs **spin something** vs **leave**, using returned values.

Still open from owner: exact names of talk modes; whether escalate is a first-class action.

### Verification — ingress event_type (2026-08-06)

**Question:** Does ingress emit distinct `user.cancel` / `user.correction` / `user.confirmation`, or uniform `user.message`?

**Fact (code):** **Uniform `user.message`.**

Evidence:
- `event_from_user_utterance` hardcodes `event_type: "user.message"` (`event_admission.rs` ~205).
- `admit_event` does not classify cancel/correction/confirmation; it stores whatever `NormalizedEventV1.event_type` it is given and optionally runs Conductor on payload `text`.
- Workspace search: no producer emits `user.cancel` / `user.correction` / `user.confirmation` (only Blueprint aspirational list §4.1).
- `decide_deterministic`: anything other than `user.message` → Escalate; all chat text shares one type.

### Decision — `classify.pending_response` shape (2026-08-06)

**Question:** Does the Call return `{kind, confidence}` with margin threshold in the Sol program, or `{kind}` with threshold inside the target?

**Decision: `{kind, confidence}` out of the target; threshold lives in the program.**

Rationale (owner):
- Threshold is a tunable visible in the bag/ledger trace.
- Per-tenant / per-harness adjustment without republishing the classifier target.
- Classifier stays a pure-ish scorer; policy (accept vs abstain) stays in the harness body.

Contract sketch:
```
Call classify.pending_response@1
  in:  { text, pending_step?, catalog_hints? }
  out: { kind: str, confidence: float }   // kind ∈ cancel|correction|confirmation|other|…
Program:
  Branch confidence >= local.threshold  → use kind
  else                                  → abstain / rough_chat / re-ask
```

`local.threshold` (or `in.threshold` / page slot) is program- or tenant-provided, not buried in the Call impl.

**Caveat (design, not a reopen):** model confidence is nondeterministic → replay must INJECT the recorded `{kind, confidence}` from the ledger (Mother intent/dispatch/result), never re-call the model on replay. Threshold compare stays pure on injected values.

### Process lock — baby steps (2026-08-06)

Owner: stop the big cascade. Proceed **one concrete step at a time**.
Do not stack more Phase B decisions until the current baby step is done and verified.

**Done facts/decisions so far:**
1. Ingress event_type is uniform `user.message` (verified).
2. `classify.pending_response` returns `{kind, confidence}`; threshold in program (decided).

**Next:** pick ONE baby step only (owner + monitor agree before coding).
