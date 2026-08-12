# Aelio Agent-Loop Harness — Audited Implementation and Cutover Plan

**Status:** Executed through the default-harness integration checkpoint; production rollout remains an operator decision  
**Source proposal:** [`AELIO_HARNESS_SPEC.md`](AELIO_HARNESS_SPEC.md)  
**Target:** The authoritative Rust `/v1/turns` runtime used by every channel  
**Sol:** Explicitly out of scope  
**Primary objective:** Replace the current keyword/plan spine with a general model → tool → result loop, then make that loop the default turn authority after shadow, canary, and rollback gates pass.

---

## 1. Executive decision

The proposal's product direction is accepted: Aelio needs one general-purpose agent loop whose model receives bounded context and a tool catalog, calls tools repeatedly, observes structured results, and explicitly terminates through a gated `finish` call.

The proposal is **not directly implementable as written** in this repository. It assumes a greenfield Rust workspace and duplicates protocol, transport, registry, server, persistence, and SDK-bridge components that already exist. It also contains several conflicting invariants and security semantics. This plan resolves those gaps and integrates the new harness into the current production spine.

The implementation unit will be a new Rust crate:

```text
aelio-os/crates/aelio-agent-loop/
```

It will be called from `aelio-agent-api` behind `/v1/turns`, reuse the existing authenticated SDK bridge and admitted tenant catalog, and return the existing `TurnResult`/render contract. TypeScript remains the channel/model edge; it does not become a second execution authority.

### 1.1 Execution status (2026-08-09)

The main-harness slices of Phases 0–4 have now been implemented for the repository's local/default launch path. The deterministic loop, gateway contract, lifecycle-aware policy gate, action-bound confirmation, encrypted fenced conversation state, persistent rate limiting, scoped `memory_search`, SDK bridge, channel ingress, readiness reporting, and rollback selector are wired end to end. `pnpm start` assigns new conversations to `agent_loop` unless an explicit environment override selects another authority.

This is a code-complete integration checkpoint, not a claim that a production canary observation window has elapsed. Phase 5 remains deliberately optional. The source proposal's wider async sub-harness/Starlark work and dynamic shadow comparison are not prerequisites for making the flat native-tool loop the main harness and were not pulled into this cutover.

The evidence and residual-gap disposition are recorded in [`IMPLEMENTATION_AUDIT.md`](IMPLEMENTATION_AUDIT.md) and under [`evidence/`](evidence/). The complete local TypeScript umbrella suite now passes with `agent_loop`, including confirmation, authentication, channel identity, and isolated memory recall. Three legacy API assertions remain red in the dirty baseline; none execute the `agent_loop` authority path. They must be resolved by their owning compatibility work before the repository can claim a completely green universal regression suite.

The 2026-08-10 hardening pass closed every locally actionable acceptance item previously marked Partial: missing-input persistence, confirmation tamper evidence, deterministic overload retry, overflow compaction/retry, crash-safe invocation reservations, lifecycle continuation/transition, and cooperative cancellation. Only the real production observation/rollback record in E30 remains external to repository execution.

---

## 2. Current production path

```text
Web / WhatsApp / other channel
  → server/src/conversation-turn.ts
  → AelioRuntimeClient.submitAgentTurn()
  → Rust POST /v1/turns
  → aelio-agent-api::process_turn
  → current legacy or Sol selection
  → existing Rust SDK bridge
  → tenant SDK tool handler
```

The cutover point is `aelio-agent-api::process_turn`. The new harness must not create a second public server, a second WebSocket protocol, or a second tenant catalog.

### 2.1 Existing assets to reuse

| Concern | Existing authority | Plan |
|---|---|---|
| Public turn endpoint | `aelio-agent-api` `/v1/turns` | Reuse |
| Channel ingress/rendering | TypeScript server + `RenderFrame` | Reuse |
| LLM egress | TypeScript LLM gateway | Extend for structured tool calls/usage |
| Tenant SDK socket | Rust `aelio-agent-api::sdk_bridge` | Reuse and harden |
| Tenant catalog | `TenantDecl`, catalog install/runtime | Adapt into immutable loop manifest |
| Tool policy and invocation | existing agent/runtime gates and SDK host | Reuse through a narrow adapter |
| Durable store | existing Aelio DB/store crates | Add loop repositories; do not invent “Astrolobe tables” abstractly |
| Existing turn result | `aelio_agent::blocks::turn::TurnResult` | Preserve at API boundary |
| Existing fallback | legacy agent spine | Retain during rollout only |

### 2.2 Components not to duplicate

- No new standalone `aelio-server` crate.
- No replacement WebSocket stack in Phase 1.
- No second registry with independent truth.
- No second SDK manifest wire format.
- No TypeScript execution loop parallel to Rust.
- No Sol dependency or Sol routing in the new loop.

---

## 3. Required architecture

```text
                    ┌──────────────────────────────┐
Inbound turn ──────>│ AgentLoopService             │
                    │                              │
                    │  load/create conversation    │
                    │  acquire lease               │
                    │  assemble stable context     │
                    │  enforce limits              │
                    │       │                      │
                    │       v                      │
                    │  ModelGatewayAdapter ─────────────> TS LLM gateway
                    │       │                      │
                    │       ├─ final/finish ──> completion gate
                    │       │                      │
                    │       └─ tool calls          │
                    │              │               │
                    │              v               │
                    │        EffectGate            │
                    │              │               │
                    │              v               │
                    │        ToolHostAdapter ────────────> existing SDK bridge
                    │              │               │
                    │        append results ───────┘
                    └──────────────────────────────┘
```

### 3.1 Core modules

```text
aelio-agent-loop/src/
├── lib.rs                 public service and types
├── engine.rs              loop state machine
├── model.rs               provider-neutral request/response contract
├── context.rs             canonical append-only message log
├── manifest.rs            immutable per-conversation tool projection
├── tools.rs               kernel tools and dispatch
├── completion.rs          finish validation
├── confirmation.rs        action-bound confirmation state
├── effects.rs             authorization adapter and effect journal
├── budget.rs              token/call/time accounting
├── progress.rs            mechanical stall detection
├── lease.rs               conversation serialization
├── persistence.rs         repository traits and durable records
├── errors.rs              closed failure taxonomy
├── trace.rs               structured events/metrics
└── prompts/system.md      static prompt source
```

Provider-, bridge-, and database-specific implementations stay in integration crates. The core crate depends on traits and deterministic data types so its acceptance suite uses fakes only.

### 3.2 Loop state machine

```text
AcquireLease
  → LoadSession
  → DrainInbox
  → CheckCancellationStateBudgetProgress
  → CompactIfRequired
  → CallModel
  → PersistAssistantEvent
      ├─ finish only → CompletionGate → Finished or append rejection
      ├─ tool calls  → Validate → Authorize/Confirm → Dispatch → PersistResults → loop
      ├─ plain text  → protocol nudge → loop
      └─ invalid     → structured protocol error → loop/terminate by limit
```

Every transition that changes durable conversation state must be persisted before the next externally visible action.

---

## 4. Audit resolutions and normative corrections

This section overrides conflicting language in the source proposal.

### A-01 — Prefix stability must be defined over provider prompt segments

The statement “serialized request N is a strict byte-prefix of request N+1” is generally false for JSON envelopes: closing delimiters, request IDs, model parameters, and cache markers change. The enforceable invariant is:

> Immutable prompt segments (tool catalog, system prompt, bootstrap) are byte-identical for a pinned manifest; previously emitted conversation content blocks are byte-identical and remain in the same order until an explicitly recorded rewrite boundary.

Tests compare canonical segment bytes and provider cache keys, not raw whole-request prefixes.

Allowed rewrite boundaries are only:

- compaction;
- secret redaction/scrubbing;
- progress-constrained recovery, if enabled.

Each emits `ContextRewrite { reason, before_hash, after_hash }`.

### A-02 — Kernel tool ordering is canonical, not “always first” and globally sorted

Canonical order is two stable partitions:

1. kernel tools in a versioned fixed order;
2. tenant tools sorted by canonical tool identity.

The manifest hash includes the ordering version.

### A-03 — Mixed `finish` and business calls are invalid

The source pseudocode would accept `finish` and silently skip sibling calls. New rule:

- `finish` must be the only tool call in a model response;
- a response mixing `finish` with other calls receives `MixedFinishAndActions` and dispatches nothing.

This prevents ambiguous completion and lost effects.

### A-04 — Failure acknowledgement must be structured

The kernel cannot reliably decide whether prose “acknowledges” a failed effect. `finish` receives:

```json
{
  "message": "...",
  "status": "completed|partial|blocked|refused",
  "resolved_effect_ids": [],
  "unresolved_effect_ids": []
}
```

The completion gate verifies IDs against the effect journal. It never analyzes prose.

### A-05 — Confirmation is kernel-issued and invocation-bound

The model does not authorize an effect by calling `confirm_with_user`. When a gated call requires confirmation, the kernel parks a canonical pending invocation containing:

```text
tenant + conversation + tool version + canonical args hash + effect class + expiry + nonce
```

The user confirms that exact digest. One approval can release exactly one matching invocation. Changed arguments require a new confirmation. Denial, expiry, lifecycle narrowing, or manifest drift voids it.

`confirm_with_user` may remain a presentation primitive, but never an authorization primitive.

### A-06 — Phase 1 cannot be exposed without authorization

An internally tested loop may initially use a deny-all/fake gate. It must not receive production traffic until the real effect gate and action-bound confirmation pass Phase 2. “Loop without auth” is a development slice, not a deployable mode.

### A-07 — MCP-shaped is compatibility, not compliance

The existing protocol may adopt selected MCP-compatible content shapes. It must not claim MCP compliance until initialization, capability negotiation, method/error semantics, versioning, and conformance tests pass. Deviations are recorded in `docs/harness_new/PROTOCOL_DEVIATIONS.md`.

### A-08 — Provider capabilities are explicit

The loop requires structured assistant content, stable call IDs, usage, and stop reasons on the **internal** gateway contract (`AgentModelRequestV2` / response). The provider-facing transport is **ReAct JSON text** (`tool_transport: "react_json"`): the TypeScript gateway encodes tools as a prompt catalog, sends plain chat messages (no provider `tools` / `tool_calls` / `role:tool`), and parses a strict `{"thought","actions":[...]}` JSON object back into structured `tool_call` blocks for the kernel. Native provider function-calling is disabled on the agent path (`native_tools: false`). Kernel `finish` / effect gate / confirmation remain structured and unchanged.

### A-09 — Parallel dispatch is effect-aware

Calls from one response are not blindly joined:

- independent `Read` calls may run concurrently;
- writes run in response order unless their manifests explicitly declare safe independence;
- calls with overlapping resource keys are serialized;
- if any call requires confirmation, that call is parked and unrelated safe reads may complete; no later write is dispatched past the parked write;
- result blocks are appended in original call order.

### A-10 — Persistence has an explicit consistency contract

The source assumes atomic writes without mapping them to the current store. Required repository operations are:

- append message/event with optimistic conversation version;
- atomically record effect outcome plus lifecycle transition and counters;
- compare-and-set lease acquire/renew/release;
- atomically create/consume/void confirmation;
- idempotently record tool invocation correlation;
- append inbox message and drain by ordered sequence.

If the current store cannot provide one operation, that phase remains gated.

### A-11 — Secret handling is provider-aware

Secrets cannot be removed from an already-sent provider context. They are minimized before dispatch, never persisted, and replaced with a stable redaction block in subsequent requests at an explicitly logged rewrite boundary. Secret-bearing tools should prefer opaque server-side handles. Tests scan persisted fixtures, traces, errors, and request dumps.

### A-12 — Starlark is gated on an async-host feasibility spike

The Rust Starlark implementation and current tool bridge may not support suspending and resuming async host calls as assumed. Before Phase 4, implement a spike proving parse → analyze → authorize-zero-dispatch → bounded host invocation. If safe async execution cannot be proven, `run_program` remains disabled and the main loop ships without it.

### A-13 — “Astrolobe” maps to concrete Aelio DB repositories

No code may depend on an undefined storage brand/interface. Each vector, text, graph, temporal, lease, message, effect, and confirmation operation must map to an existing Aelio DB API or a new tested repository method.

### A-14 — Tool identity is version-pinned

Conversation manifests pin `(tenant_id, catalog_hash, tool_id, tool_version, schema_hash, effect_metadata_hash)`. Re-registration affects new conversations only. Existing conversations either retain an invokable pinned version or fail closed with `PinnedToolUnavailable`; they never silently invoke a newer schema.

### A-15 — Main-harness cutover is reversible

Add a new authority mode instead of repurposing `legacy` or `sol`:

```text
AELIO_HARNESS_MODE=agent_loop | legacy | auto
```

During migration, `auto` moves through shadow → canary → default. Sol is ignored by the new implementation and removed from this mode's decision tree. Emergency rollback selects `legacy` for new turns; already-dispatched effects are never replayed.

### A-16 — Budget units are model/provider-specific and bounded

Defaults cannot assume a 400k context or token budget. At conversation creation:

```text
effective limits = min(tenant configuration, provider capabilities, global safety ceilings)
```

Input, cached input, output, retries, compaction, tool calls, wall clock, and child reservations are accounted separately.

### A-17 — `finish` is the normal exit, not the only possible outcome

The complete outcome algebra is:

```text
Finished | WaitingForUser | Refused | Cancelled | Exhausted | Failed
```

Only `Finished` comes from accepted `finish`. Confirmation and missing required user data return durable `WaitingForUser`; hard kernel conditions return typed terminal outcomes with a non-empty user message.

### A-18 — Model prose is never an authorization/test oracle

All gating tests assert invocation counts, effect journals, state versions, hashes, and emitted event types. Scenario evaluations may inspect final user-facing content only for non-emptiness, leakage, and schema compliance—not semantic correctness inferred from arbitrary prose.

---

## 5. Protocol contracts

### 5.1 Model gateway v2

Add versioned request/response types shared by Rust and the TypeScript LLM gateway.

```rust
struct AgentModelRequestV2 {
    request_id: String,
    model: String,
    system: Vec<ContentBlock>,
    messages: Vec<AgentMessage>,
    tools: Vec<ToolDefinition>,
    tool_choice: ToolChoice,
    limits: ModelLimits,
    cache: CacheHints,
}

struct AgentModelResponseV2 {
    id: String,
    content: Vec<AssistantBlock>,
    stop_reason: StopReason,
    usage: ProviderUsage,
}

enum AssistantBlock {
    Text { text: String },
    ToolCall { id: String, name: String, arguments: Value },
}
```

Requirements:

- strict unknown-field rejection at trust boundaries;
- max request/response byte sizes before deserialization;
- stable call IDs and exact tool-result correlation;
- no tool-call extraction from markdown/JSON text;
- retries preserve logical request identity while recording attempt identity;
- provider capability response reports context window, tool support, cache support, and streaming support.

### 5.2 Loop manifest

The loop manifest is a frozen projection of the admitted catalog, not a new source of truth.

```rust
struct LoopManifest {
    tenant_id: TenantId,
    catalog_hash: Hash,
    prompt_version: String,
    ordering_version: String,
    tools: Vec<LoopTool>,
    persona: SanitizedPersona,
    hash: Hash,
}
```

Manifest admission must validate names, descriptions, JSON Schema, result caps, effect class, idempotency metadata, progress labels, sensitivity annotations, and version identity. Heuristic name/effect mismatches are hard errors only for high-confidence destructive/security verbs; ambiguous cases become warnings plus tenant review.

### 5.3 Tool-result envelope

```json
{
  "invocation_id": "...",
  "tool": "...",
  "status": "ok|error|unknown|denied|confirmation_required",
  "data": {},
  "error": {"class": "...", "retryable": false, "detail": "..."},
  "truncated": false,
  "effect_record_id": "..."
}
```

Raw tenant errors are mapped to closed public classes. Stack traces and internal identifiers never enter the model or customer response.

---

## 6. Persistence model

Use append-oriented records with optimistic versions. Exact physical layout is chosen after repository capability tests.

| Record | Identity | Required properties |
|---|---|---|
| Conversation | tenant + conversation | pinned manifest, status, version, budgets, lifecycle state |
| Message/event | conversation + monotonic seq | append-only, canonical hash, sensitivity metadata |
| Inbox | conversation + arrival seq | durable, ordered, deduplicated ingress ID |
| Lease | conversation | fencing token, holder, expiry, CAS |
| Invocation | tenant + logical invocation ID | args hash, pinned tool, dispatch state, outcome |
| Effect | effect record ID | invocation link, class, outcome, redacted args hash |
| Confirmation | confirmation ID | canonical action digest, expiry, state, single-use CAS |
| Context rewrite | conversation + seq | reason, old/new hash, compaction provenance |
| Budget ledger | conversation + entry seq | immutable charges/reservations/refunds |

Fencing tokens are mandatory: an expired lease holder must not append after a takeover.

---

## 7. Phased implementation

No phase may route external traffic until its gate passes. Every phase ends with `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.

### Phase 0 — Baseline, traceability, and feasibility

Deliverables:

1. Record clean/dirty worktree boundaries; never overwrite unrelated current changes.
2. Add a requirement matrix mapping every source-spec section/test to this plan.
3. Capture baseline Rust and TypeScript test results.
4. Prove the TypeScript gateway can transport native tool calls and provider usage.
5. Prove the current store can provide fenced leases and atomic effect/state writes, or design the minimal extensions.
6. Decide supported provider set for the first release.

Gate P0:

- baseline failures are documented and distinguished from new failures;
- no wire/storage unknown remains for Phase 1;
- gateway v2 round-trip fixture passes Rust ↔ TypeScript compatibility tests;
- implementation file ownership avoids unresolved overlap with current user edits.

Checkpoint: `P0_ARCHITECTURE_ACCEPTED`.

### Phase 1 — Deterministic loop core (not production-routable)

Implement:

- new crate and closed core types;
- `MockModel`, `MockToolHost`, `FakeClock`, deterministic IDs;
- context segments and canonical hashing;
- frozen loop manifest projection;
- loop state machine;
- kernel tools `finish` and internal protocol notice;
- flat catalog mode only;
- budgets, hard terminators, provider retry classification;
- effect-aware parallel read dispatch;
- explicit progress monitor;
- structured traces;
- result size caps.

Required tests:

- static system segment byte identity;
- canonical manifest ordering across randomized insertion orders;
- append-only event sequence;
- rewrite only through tagged API;
- native tool-call/result correlation;
- mixed finish/actions dispatches zero calls;
- bare prose never returns `Finished`;
- protocol violation limit exactness;
- tool timeouts/panics become structured results;
- read calls parallelize while ordered writes serialize;
- result order equals model call order under jitter;
- retry attempts do not consume model-turn count but do consume wall clock;
- all exhaustion outcomes contain a safe non-empty user message;
- repeat-call, oscillation, barren-turn, and search-thrash detectors are mechanical;
- progress detection makes zero additional model calls.

Gate P1:

- core crate tests green under 100 randomized scheduler runs;
- no external network in tests;
- no production route references the new service;
- workspace tests/clippy green.

Checkpoint: `P1_LOOP_CORE_GREEN`.

### Phase 2 — Security, state, confirmations, and durability

Implement:

- adapter from admitted catalog/effect metadata to loop manifest;
- default-deny effect gate;
- lifecycle-state capability derivation;
- pure deterministic policy adapter;
- action-bound confirmation parking/resume;
- fenced conversation lease and inbox;
- invocation idempotency/correlation journal;
- atomic effect + transition + counter write;
- PII/secret/no-persist handling;
- tenant/end-user scoped memory repository interfaces;
- per-user, conversation, tenant, and per-tool rate limits.

Required tests:

- unauthorized call produces zero SDK dispatches;
- malicious user text/tool result/tool description cannot widen capability;
- default deny when no policy matches;
- deny > confirm > allow composition;
- effect/state/counter atomicity under injected crash at every write boundary;
- transition requires recorded successful effect;
- narrowing voids pending confirmations;
- confirmation digest binds tenant, conversation, versioned tool, args, effect, expiry;
- confirmation is single-use under 20 concurrent consumers;
- altered args after confirmation require new approval;
- 20-way lease race yields one fenced winner;
- expired holder cannot commit after takeover;
- inbox never splits tool-call/result pairing;
- non-idempotent unknown outcome is never automatically retried;
- raw PII/secret literal absent from every persisted fixture, trace, dump, and error;
- cross-tenant and cross-user retrieval returns zero foreign records;
- all four rate-limit scopes block before SDK dispatch.

Gate P2:

- security suite green;
- fault-injection suite green;
- storage restart/resume test is byte- and effect-equivalent to control;
- threat-model review contains no unresolved critical/high finding;
- workspace tests/clippy green.

Checkpoint: `P2_SECURITY_DURABILITY_GREEN`.

### Phase 3 — Integration behind `/v1/turns`, shadow mode

Implement:

- `AgentLoopService` in `AppState`;
- gateway v2 client using existing TypeScript LLM gateway;
- tool-host adapter using existing Rust SDK bridge;
- conversion from loop outcomes to existing `TurnResult` and `RenderFrame`;
- new `agent_loop` and `shadow_agent_loop` authority modes;
- shadow execution with effects disabled and deterministic comparison telemetry;
- channel-visible working/acting/confirmation/final events without intermediate model prose;
- startup readiness checks for model/tool/persistence capabilities.

Required tests:

- widget and WhatsApp enter the same Rust loop path;
- shadow mode dispatches zero tenant effects;
- final response preserves existing API schema;
- only accepted finish content reaches customer output;
- progress label comes only from admitted manifest;
- SDK disconnect/reconnect preserves invocation identity;
- manifest drift does not alter active conversation;
- pinned unavailable tool fails closed;
- gateway unavailable/auth/invalid/context-limit errors follow the closed taxonomy;
- legacy mode behavior remains regression-equivalent.

Gate P3:

- all existing public API, widget, WhatsApp, SDK, lifecycle, replay, and drain tests green;
- shadow run on scenario corpus has zero unauthorized-effect deltas and zero empty outcomes;
- no new production effects occur in shadow mode;
- workspace Rust and repository TypeScript suites green.

Checkpoint: `P3_SHADOW_READY`.

### Phase 4 — Canary and default cutover

Rollout stages:

1. internal test tenant, 100% agent loop;
2. allowlisted tenant, read-only conversations;
3. allowlisted tenant, reversible writes;
4. confirmed high-stakes effects;
5. 1%, 5%, 25%, 50%, 100% of new conversations.

Conversation authority is pinned at creation; never switch a live conversation between engines.

Promotion criteria at every stage:

- unauthorized dispatch count = 0;
- duplicate non-idempotent effect count = 0;
- cross-tenant data leak count = 0;
- empty customer reply count = 0;
- confirmation bypass count = 0;
- loop exhaustion and no-progress rates within agreed thresholds;
- p95 latency and cost within budget;
- rollback drill completed.

Rollback:

- stop assigning new conversations to `agent_loop`;
- retain loop state for forensic replay;
- allow already-running turns to reach a safe boundary or cancel without retrying in-flight writes;
- set new conversations to `legacy`;
- never translate/replay an incomplete loop effect into the legacy spine automatically.

Gate P4:

- 100% default held for a defined observation window;
- all safety counters remain zero;
- operational alerts and runbooks verified;
- legacy remains an explicit emergency fallback.

Checkpoint: `P4_AGENT_LOOP_DEFAULT`.

### Phase 5 — Optional scale features

Only after real measurements justify them:

- progressive tool discovery and full-schema retrieval;
- compaction with decision/effect preservation;
- sub-harnesses and budget reservation trees;
- Starlark `run_program` after feasibility/security gate;
- declared flows as coarse tools;
- catalog enrichment with tenant review;
- promotion/evidence workflows;
- multi-node SDK socket routing.

Each feature receives its own feature flag, acceptance suite, shadow period, and rollback path. None blocks making the basic agent loop primary.

---

## 8. End-to-end scenario matrix

Tests use a deterministic fake provider for CI and an optional real-provider evaluation lane. Assertions target effects and state.

| ID | Scenario | Required assertions |
|---|---|---|
| E01 | Simple informational answer | accepted `finish`; zero SDK calls; non-empty frame |
| E02 | One read tool | exactly one version-pinned call; result correlated; finish follows |
| E03 | Two independent reads | overlap in execution; results appended in call order |
| E04 | Two writes | dispatch in declared order; no unsafe parallelism |
| E05 | Missing argument | no SDK dispatch until user supplies value; durable waiting state |
| E06 | Reversible write | policy allow recorded; exactly one effect |
| E07 | Financial action | zero dispatch before exact confirmation; one after confirmation |
| E08 | Confirmation args altered | old confirmation rejected; zero altered dispatches |
| E09 | User denies confirmation | pending action voided; zero dispatches |
| E10 | Tool error then recovery | error structured; changed corrective call allowed; no identical retry loop |
| E11 | Tool injection payload | capability unchanged; unauthorized call count zero |
| E12 | User prompt injection | capability unchanged; unauthorized call count zero |
| E13 | Tenant description injection | manifest rejected before conversation |
| E14 | Oversized tool result | transport cap/explicit truncation; no silent partial truth |
| E15 | Provider 429/overload | bounded jittered retries; one logical turn charge |
| E16 | Provider context overflow | one tagged compaction and retry; then typed exhaustion |
| E17 | SDK disconnect during idempotent read | safe reconnect behavior; correlation preserved |
| E18 | SDK disconnect during non-idempotent write | outcome unknown; no automatic replay |
| E19 | Duplicate inbound message | one durable user event and one effect sequence |
| E20 | Concurrent inbound messages | one lease holder; inbox ordered at boundary |
| E21 | Process crash after dispatch | restart does not duplicate effect; outcome reconciled |
| E22 | Manifest changes mid-conversation | pinned tools/system bytes unchanged |
| E23 | Lifecycle login | capability widens only after recorded success |
| E24 | Lifecycle logout/TTL | capability narrows and confirmations void |
| E25 | Repeat-call stall | nudge, constraint, typed stop before max turns |
| E26 | Cancellation | in-flight effect lands/records; no new dispatch; user informed |
| E27 | Secret argument | absent from durable stores/logs/dumps next boundary |
| E28 | Tenant isolation | zero foreign records across catalog, memory, effects |
| E29 | Channel parity | web and WhatsApp produce equivalent effects and terminal status |
| E30 | Emergency rollback | new conversation uses legacy; old writes not replayed |

Real-provider evaluations additionally measure tool-selection success, finish-protocol adherence, turns, cached/uncached tokens, latency, and cost. They never gate deterministic CI on exact prose.

---

## 9. Verification commands and evidence

Every checkpoint stores command, commit/worktree hash, environment, exit code, and artifact paths.

```bash
cargo test --manifest-path aelio-os/Cargo.toml --workspace
cargo clippy --manifest-path aelio-os/Cargo.toml --workspace --all-targets -- -D warnings
pnpm typecheck
pnpm lint
pnpm test:all
pnpm test:e2e
pnpm test:phase3
pnpm test:phase4
pnpm test:phase5
pnpm test:phase6:lifecycle
```

Additional harness commands to add:

```bash
cargo test --manifest-path aelio-os/Cargo.toml -p aelio-agent-loop
pnpm test:agent-loop:contract
pnpm test:agent-loop:e2e
pnpm test:agent-loop:chaos
pnpm test:agent-loop:shadow
```

Evidence directory:

```text
docs/harness_new/evidence/<checkpoint>/
├── summary.md
├── commands.jsonl
├── test-results/
├── request-fixtures/
├── traces/
├── fault-injection/
└── known-limitations.md
```

Secrets and raw PII must be scrubbed before evidence is written.

---

## 10. Observability and operational gates

Minimum structured events:

- `LoopStarted`, `LoopResumed`, `LoopFinished`;
- `ModelAttempted`, `ModelCompleted`, `ModelRetried`, `ModelFailed`;
- `ToolProposed`, `ToolDenied`, `ConfirmationRequired`, `ToolDispatched`, `ToolCompleted`;
- `EffectRecorded`, `LifecycleTransitioned`;
- `BudgetCharged`, `BudgetExhausted`;
- `ProgressStrike`, `ContextRewrite`;
- `LeaseAcquired`, `LeaseRenewed`, `LeaseLost`;
- `RollbackSelected`.

Required alert invariants:

- any unauthorized dispatch;
- any effect without a matching gate decision;
- any successful lifecycle transition without an effect record in the same atomic commit;
- any confirmation consumption with a mismatched action digest;
- any append using a stale fencing token;
- any duplicate successful non-idempotent invocation;
- any raw secret in persistence or INFO logs;
- any customer-visible intermediate model text.

---

## 11. Threat model checklist

- Prompt injection in user input, tenant descriptions, tool results, and retrieved memory.
- Tenant attempts to misclassify destructive tools as reads.
- Model fabricates tool names, call IDs, effect completion, state, or confirmation.
- Replay/double-submit of inbound messages, confirmations, and SDK results.
- Socket loss at every boundary before/after dispatch and persistence.
- Stale-node writes following lease expiry.
- Cross-tenant catalog, memory, invocation, effect, and trace leakage.
- Resource exhaustion through huge schemas/results, fan-out, retries, and long loops.
- Secret propagation into prompts, persistence, traces, dumps, and error messages.
- Provider response corruption, unsupported tool semantics, and partial streams.
- Unsafe parallel writes and result-order nondeterminism.
- Rollback duplicating or losing already-dispatched effects.

No critical/high threat may remain “accepted” without an explicit owner and production gate.

---

## 12. Requirement traceability

| Source sections | Destination |
|---|---|
| §1, §4, §6, §9 | Phases 1 and 3: loop, prompt, finish gate |
| §3, §7 | Existing SDK/catalog adapters; Protocol §5 |
| §5, §13 | Context invariant A-01; optional compaction |
| §8 | Optional Phase 5 sub-harnesses |
| §10 | Optional Phase 5 after async feasibility spike |
| §11, §14 | Phase 2 scoped repositories and persistence mapping |
| §12, §30, §31 | Phase 2 effect gate, lifecycle, policy, confirmation |
| §15, §27, §29 tests | Phase gates plus E2E matrix |
| §18 | Phase 3 channel events; finish-only customer text |
| §19 | Phase 2 leases, fencing, inbox, interrupts |
| §20 | Phases 1–3 failure taxonomy, retries, caps, circuits |
| §21 | Phase 2 PII/secrets/erasure |
| §22 | Phase 2 rate limits |
| §23–§25 | Phases 3–5 operations/deployment |
| §32 | Optional Phase 5 enrichment with review |

---

## 13. Definition of done

The new harness is complete only when all are true:

1. All new conversations use `agent_loop` by default in production configuration.
2. Every channel reaches the same Rust loop authority.
3. Native structured tool calling is used; no prose parsing.
4. Every external effect has an admitted versioned tool, gate decision, invocation record, and outcome.
5. High-stakes effects require a single-use action-bound confirmation.
6. Crash/reconnect/retry tests prove no duplicate non-idempotent effects.
7. Fenced leases prevent concurrent conversation mutation.
8. Tenant/end-user isolation tests return zero leakage.
9. Secrets/PII pass persistence and log scans.
10. Finish, wait, refuse, cancel, exhaust, and fail outcomes are explicit and customer-safe.
11. Existing public API/channel/SDK tests remain green.
12. Rust tests and clippy with warnings denied are green.
13. TypeScript typecheck, lint, unit, integration, and E2E suites are green.
14. Canary metrics meet promotion thresholds and a rollback drill succeeds.
15. Known limitations document contains no unowned critical/high item.

Until these conditions hold, the correct product description is “agent-loop harness in staged rollout,” not “the main harness.”

---

## 14. Immediate execution order

1. Finish baseline commands and record pre-existing failures.
2. Add the full source-spec → plan/test traceability checklist.
3. Define and test Model Gateway v2 fixtures in Rust and TypeScript.
4. Prove fenced lease and atomic effect/transition storage operations.
5. Scaffold `aelio-agent-loop` and its deterministic fakes.
6. Implement Phase 1 strictly behind a non-production feature/mode.
7. Stop at every phase gate on any red or weakened test.
