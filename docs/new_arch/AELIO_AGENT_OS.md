# Aelio Agent OS — Design Note

**Status:** normative metaphor (v0.1)  
**Companions:** [`AELIO_L0_GLOSSARY.md`](./AELIO_L0_GLOSSARY.md), [`AELIO_L1_GLOSSARY.md`](./AELIO_L1_GLOSSARY.md)  
**Audience:** designers and implementers building a production open-source agent runtime

---

## 1. One-sentence definition

**Aelio is a typed agent operating system:** a domain-blind kernel (L0), system services (L1), durable userland routines (flows/procedures), a multimodal filesystem (Sunjet), and an LLM used only as a **constrained routine author** — never as the unsupervised kernel.

---

## 2. Metaphor map

| Classic OS | Aelio | Notes |
|---|---|---|
| ISA / syscalls | **L0** combinators, pure ops, effects | Closed; tenants cannot extend |
| libc / daemons | **L1** abilities via `Call{id}` | Sense, Understand, Recall, Invoke, … |
| Processes / programs | **Flows & procedures** (Op trees) | Authored or learned; stored & retrieved |
| Filesystem | **Sunjet** logical spaces | Messages, memories, procedures, docs, … |
| Scheduler | **Turn loop** + `Park` / `Schedule` | No `Sleep`; workers must free |
| Package install | **Learn.Propose → TypeCheck → Promote** | Slow to install, fast to uninstall (`Demote`) |
| Root / MAC | **Policy** + decision ledger | Executor invariant around effects |
| Init / boot | Tenant catalog load | Tools, states, personalities, policies |
| Shell | Channel ingress (web, WhatsApp, SDK) | Transport only — not intelligence |

```text
┌─────────────────────────────────────────────────────────┐
│  Userland     Flows / Procedures (Op trees in Sunjet)   │
├─────────────────────────────────────────────────────────┤
│  Services     L1 abilities (Call targets)               │
├─────────────────────────────────────────────────────────┤
│  Kernel       L0-A combinators · L0-B pure · L0-C fx    │
├─────────────────────────────────────────────────────────┤
│  Filesystem   Sunjet (rows, text, vector, graph)        │
├─────────────────────────────────────────────────────────┤
│  LLM          ProposePath / cold Understand / Express   │
│               (privileged author — not the kernel)      │
└─────────────────────────────────────────────────────────┘
```

---

## 3. Non-goals (absolute)

1. **LLM-as-kernel** — freeform model control of every turn. Breaks replay, policy, and cost.
2. **Domain syscalls** — no `SendOtp` / `QueryCRM` inside L0. Domain is L1 + tenant tools.
3. **Blocking sleep** — waiting is always `Park` (end turn, persist, resume).
4. **Silent coercion / ambiguous null** — totality with `ReasonCode` only.
5. **Unbounded composition search** — L0-A stays small so Learn.Compose stays finite.
6. **Personality in retrieval keys** — tone must not invalidate learned procedures.

---

## 4. Trust boundary

| Zone | Who may change it | How |
|---|---|---|
| L0 ISA | Platform maintainers only | Glossary + code; closed set |
| L1 ability contracts | Platform (+ versioned) | Glossary; multiple substrates OK |
| Tenant tools / states / policies | Tenant via SDK | Registration; capability tags |
| Promoted procedures | Runtime after gates | Evidence thresholds; immutable versions |
| Effectful path promotion | Tenant approval when required | `Learn.Promote` + policy |
| Raw LLM prose to user | `Express.*` only | Terminal; not fed back as control |

**Invariant:** anything that mutates the world (`Invoke.Call`, `State.ProposeTransition`, durable Remember) passes **Policy** as an executor wrap — composers cannot omit it.

---

## 5. Turn = syscall batch

One inbound user message (or proactive tick) = **one turn**:

```text
hydrate filesystem state
  → Sense (read cockpit)
  → flow gate (resume beats utterance)
  → Understand / triage
  → Recall? (filesystem read)
  → Learn.LookupTier (load program or compose/propose)
  → eval Op tree (L0 kernel + L1 Calls)
  → Express
  → commit filesystem + ledger
  → Park or complete
```

The LLM may run **inside** specific Calls (Understand, ProposePath, Synthesize). It does not own the batch scheduler.

---

## 6. Promote = install

| Step | OS analogy |
|---|---|
| `Learn.ProposePath` / `Compose` | Build package from declared APIs |
| `Learn.TypeCheck` | Linker / typecheck |
| Supervised execute + score | Test install |
| `Learn.Propose` | Staging |
| `Learn.Promote` | Install to PATH (immutable version) |
| `Learn.Demote` / `Registry.Invalidate` | Uninstall / recall on dependency break |

**Asymmetry:** expensive to promote, cheap to demote. Wrong installed routines are worse than missing ones.

---

## 7. Filesystem roles (Sunjet)

| Space (illustrative) | OS role |
|---|---|
| `flow_instances` / `states` | Process control blocks |
| `procedures` / `proposals` | Installed & staged programs |
| `memories` / `documents` | User & system files |
| `turns` / `messages` | Journal / tty log |
| `idempotency` / once-keys | At-most-once effect tokens |
| ledger / audit | Kernel audit log |

Sense does **not** store; it **reads** a snapshot assembled from these + the clock + the request.

---

## 8. Design participation rule

1. Discuss → decide → glossary row → then code.  
2. PRs cite glossary ids (`L0.Park`, `L1.Sense.Session`, …).  
3. No silent new kernel or service concepts.

---

## 9. Essence

**Retrieve a routine if you have one; compose or ask the LLM to propose one if you do not; install only what typechecks and earns evidence.** That is the Agent OS.
