# Aelio Agent-Loop Harness — Implementation Audit

**Audit date:** 2026-08-10  
**Source:** [`AELIO_HARNESS_SPEC.md`](AELIO_HARNESS_SPEC.md)  
**Execution plan:** [`AELIO_HARNESS_IMPLEMENTATION_PLAN.md`](AELIO_HARNESS_IMPLEMENTATION_PLAN.md)  
**Scope:** The new provider-native agent loop and its integration as the repository launcher's default harness. Sol is excluded.

## 1. Outcome

The main, flat agent loop is implemented and wired through the existing production-shaped authority graph:

```text
channel ingress
  -> TypeScript conversation-turn adapter
  -> Rust POST /v1/turns
  -> agent_loop authority branch
  -> immutable context + pinned tool manifest
  -> model gateway v2
  -> whole-response authorization and rate preflight
  -> existing authenticated SDK bridge
  -> structured tool result
  -> repeat until an accepted finish call
  -> existing TurnResult and RenderFrame
```

`pnpm start` now chooses `agent_loop` when `AELIO_HARNESS_MODE` is absent. An explicit `legacy`, `sol`, or `auto` setting remains an operator rollback/troubleshooting override. The checkout's local `.env` now explicitly selects `agent_loop`, so both the repository default and this working environment use the new authority.

This audit supports a code-complete local/default cutover. It does not certify that an external production canary has completed its observation window.

## 2. What was implemented

| Area | Implementation | Result |
|---|---|---|
| Loop core | New `aelio-agent-loop` Rust crate with provider-neutral `Model`, `ToolHost`, and `EffectGate` traits | Complete |
| Termination | Native `finish` tool is mandatory and exclusive; plain prose or mixed finish/actions cannot terminate | Complete |
| Context | Immutable system/bootstrap/catalog segments, append-only messages, canonical hashes, explicit compaction records | Complete |
| Manifest | Kernel tools first, tenant tools canonicalized, version/hash pinned per conversation | Complete |
| Tool safety | Unknown tools, injection-like manifest text, duplicate names, invalid schemas, and destructive tools labeled as reads fail admission | Complete |
| Dispatch | Read-only calls may run concurrently with bounded parallelism; any write makes the batch sequential | Complete |
| Batch atomicity | Authorization and persistent rate-limit preflight cover the whole assistant response before any SDK dispatch | Complete |
| Completion | A failed latest tool result cannot be reported as `completed`; `blocked` and `partial` remain explicit outcomes | Complete |
| Limits | Model turns, tool calls, wall time, tool timeout, parallel reads, result bytes, and progress stalls are bounded | Complete |
| Lifecycle policy | Current lifecycle state derives granted capabilities; existing deterministic policy evaluation composes deny before confirm before allow | Complete |
| Confirmation | Pending writes bind tenant, conversation, versioned tool, canonical args, effect class, expiry, and nonce; approval is single-use | Complete |
| Durability | AES-256-GCM conversation state, tenant/conversation AAD, hashed keys, CAS lease, fencing, stale-holder rejection, replay cache | Complete |
| Rate limiting | Persistent fixed-window tenant, user, conversation, and per-tool reservations occur before dispatch | Complete |
| Model gateway | Strict version-2 request/response schemas, capability handshake (`tool_transport: react_json`), ReAct JSON provider transport, size caps, and bounded retry classes | Complete |
| SDK execution | Existing authenticated SDK bridge is reused through a blocking-safe adapter and stable invocation idempotency key | Complete |
| API boundary | Existing `/v1/turns`, `TurnResult`, render frame, mode trace, and channel ingress contracts are retained | Complete |
| Operations | Readiness reports model-gateway configuration; launcher provides stable state key and explicit authority banner/toggle | Complete |
| Missing input | Required arguments park the whole response in encrypted durable state; the next user turn reconstructs the invocation | Complete |
| Context overflow | A typed provider overflow causes exactly one tagged compaction/retry without charging the failed model call | Complete |
| Crash recovery | Turn/batch/ordinal invocation intent is persisted before dispatch; provider call IDs are correlation-only | Complete |
| Cancellation | Fence-bound cooperative cancellation blocks every not-yet-started effect and records an already-running effect | Complete |
| Lifecycle continuation | Successful tool continuations are conversation/state scoped; verified evidence applies an idempotent admitted transition before widening | Complete |
| Memory search | Explicit user facts are captured asynchronously; model-requested `memory_search` is tenant/subject scoped, bounded, and enters context only as a tool result | Complete for search |

## 3. Critical invariant audit

| Invariant | Evidence |
|---|---|
| No business effect before full-batch policy approval | Gate evaluates every call before `ToolHost::preflight`; a deny/confirm decision aborts the complete batch |
| No partial batch on rate exhaustion | All four persistent scopes are reserved in preflight before the first SDK invocation |
| No model-issued authorization | Only the kernel creates and verifies the action digest; model calls merely request an effect |
| No confirmation replay | Pending confirmation is consumed under the fenced conversation lease; a later `yes` has no parked call to execute |
| No capability widening from text | Capability set comes only from the current admitted lifecycle envelope; user/tool strings are never interpreted as grants |
| No manifest drift in a conversation | Full prompt/tool manifest is stored with the conversation and active runtime tools must match pinned name and version |
| No duplicate effect from duplicate ingress | Exact request hash replays cached result; reused turn ID with altered payload returns conflict |
| No stale writer after lease takeover | CAS fence is included in encrypted-record AAD and commit validation |
| No hidden prose completion | Customer text comes from an accepted structured `finish` call |
| No unbounded tool output | Normal and post-confirmation invocation paths both use the same bounded result constructor |
| Replay does not require the model | Duplicate replay and ambiguous confirmation return before gateway capability negotiation |
| Sol does not participate | The new path branches directly to `process_agent_loop_turn`; it neither imports nor falls through to Sol execution |

## 4. Acceptance scenario disposition

`Pass` means a focused automated or live end-to-end check exists. `Covered` means the invariant is exercised by an adjacent lower-level test or existing bridge test. `Partial` is a real residual item and is not represented as complete.

| ID | Disposition | Evidence or residual work |
|---|---|---|
| E01 | Pass | Deterministic mock returns accepted `finish` with zero business calls |
| E02 | Pass | Live read scenario performed model -> SDK tool -> result -> finish |
| E03 | Pass | Core jitter test proves overlapping reads and stable result order |
| E04 | Pass | Core test proves writes dispatch sequentially in declared order |
| E05 | Pass | Missing required arguments durably suspend the complete batch with zero SDK dispatch; encrypted restart and end-to-end resume pass |
| E06 | Covered | Reversible allow and exactly-once dispatch are covered by policy/host tests; no separate named live fixture |
| E07 | Pass | Live confirmation scenario observed zero write before approval and one afterward |
| E08 | Pass | API integration test proves altered parked arguments change the kernel digest and cannot reuse approval |
| E09 | Pass | Live `no` path voided the pending call with zero dispatch |
| E10 | Pass | Structured error and completion-gate recovery behavior are core-tested |
| E11 | Pass | Tool-result injection cannot widen allowed tools |
| E12 | Pass | User prompt injection cannot widen allowed tools |
| E13 | Pass | Injection-like description/schema text is rejected at manifest admission |
| E14 | Pass | Oversized results become an explicit structured error on both execution paths |
| E15 | Pass | Deterministic transport fixture proves 429 -> 503 -> success uses one logical request, unique attempts, and bounded retries |
| E16 | Pass | Provider overflow is typed, compacted once, retried once, persisted as a rewrite, and then exhausts safely if still oversized |
| E17 | Covered | Existing SDK reconnect/correlation behavior is retained |
| E18 | Covered | Non-idempotent unknown outcomes are not automatically replayed by the loop |
| E19 | Pass | Live exact duplicate replayed byte-equivalent result; altered duplicate returned conflict |
| E20 | Pass | Store tests cover 20-way lease contention, takeover, and stale-fence rejection |
| E21 | Pass | Durable turn/batch/ordinal intent survives fresh provider call IDs; changed replay fails closed; host restart replays recorded outcome |
| E22 | Pass | Manifest hash/pinning and restart persistence are tested |
| E23 | Pass | End-to-end OTP fixture proves continuation grant, exact confirmation, recorded verification evidence, idempotent state transition, then wider tools |
| E24 | Pass | Authorization narrowing invalidates pending confirmation before dispatch |
| E25 | Pass | Repeat, oscillation, barren, and search-thrash termination are deterministic tests |
| E26 | Pass | Concurrent API cancellation is active-fence-bound; core fixture proves an in-flight write may land while every later dispatch is blocked |
| E27 | Pass | State-at-rest encryption, hashed storage keys, and tenant-bound AAD are tested |
| E28 | Pass | Store keys/AAD and runtime authority are tenant scoped; the live memory test stores conflicting preferences for two users and proves each retrieves only its own fact |
| E29 | Pass | Web and WhatsApp ingress produce equivalent agent-loop effect count and terminal status |
| E30 | Code pass / operational pending | Pure selector tests cover default and legacy rollback; a real production drill/observation record remains external |

Every locally actionable scenario that was previously `Partial` now has an implementation and automated evidence. The only remaining acceptance item that cannot be closed in this workspace is the operational portion of E30: observing a real staged production rollback. The lifecycle update is recovery-atomic rather than one physical cross-component transaction: the effect outcome is recorded first, the state command is idempotent, and a crash replay reconciles the state without re-executing the effect.

## 5. Verification record

### Focused new-harness gates

| Command/suite | Result |
|---|---|
| `cargo test -p aelio-agent-loop` | Pass: 30 tests (10 unit, 2 gateway contract, 18 integration) |
| `cargo test -p aelio-agent-api --lib` | Pass: 37 tests |
| `cargo test -p aelio-agent-api --test agent_loop_acceptance` | Pass: missing input -> confirmation -> continuation -> lifecycle transition -> wider read, plus concurrent cancellation |
| `cargo clippy -p aelio-agent-loop --all-targets --no-deps -- -D warnings` | Pass |
| `cargo clippy -p aelio-agent-api --lib --no-deps -- -D warnings` | Pass |
| `pnpm typecheck` | Pass: 16/16 workspace tasks across 12 packages |
| `pnpm test:agent-loop:contract` | Pass |
| `pnpm test:agent-loop:mock` | Pass |
| `pnpm test:agent-loop:channels` | Pass for web and WhatsApp |
| `pnpm test:authority` | Pass: Rust remains the only reachable turn decision executor |
| `AELIO_SKIP_STANDALONE=1 pnpm test:all` | Pass: all web, WhatsApp, confirmation, auth, identity, and two-user memory phases |
| `git diff --check` | Pass |

### Live end-to-end checks

- Model -> read tool -> structured result -> accepted finish returned HTTP 200 with two model calls and one SDK call.
- Exact duplicate ingress returned the cached result; same turn ID with altered request returned HTTP 409.
- Confirmation request suspended after one model call with zero SDK dispatch.
- Ambiguous `maybe` kept the request suspended with zero model and zero SDK calls.
- `yes` executed exactly one parked write and then obtained a finish; a later `yes` did not replay it.
- Encrypted conversation state resumed after runtime restart with a newer fence.
- `/ready` included a healthy `agentModelGateway` check.
- Gateway native-tool capability handshake succeeded on a post-restart turn.
- Required input parked with `tool_calls=0`, survived encrypted persistence, and resumed only after the user supplied the value.
- OTP send granted only the declared verification continuation; successful verification changed state to `authenticated`; the following protected read succeeded.
- A cancellation request arriving during a delayed model call returned immediately and replaced the delayed completion with the cancellation terminal outcome.
- 429 and 503 responses retried under one logical request; a context-overflow response triggered one recorded compaction/retry.
- Crash replay with a fresh provider call ID reused the same invocation reservation; changed arguments at that position were rejected.
- Two users stored deliberately conflicting unit preferences; each turn called the bounded `memory_search` tool and retrieved only its own fact before finishing.

### Broad regression gates

The complete non-API Rust workspace passed after isolating loopback-dependent tests and using a low-debug build profile. One kernel test remains intentionally ignored.

The complete API test binary still has three baseline compatibility failures that predate and do not select the new harness:

1. `admitted_bound_procedure_executes_through_unified_runtime_and_replays` expects `tier2` but receives `tier3`.
2. `unmaterialized_public_flow_fails_closed_before_a_runtime_proxy_effect` expects an older materialization trace for a semantic-only login flow.
3. `learning_control_plane_can_inspect_and_kill_a_promoted_procedure` expects a promoted procedure where the greeting starter path does not create one.

The post-hardening `AELIO_SKIP_STANDALONE=1 pnpm test:all` run is green under `agent_loop`. It covers the production authority graph, clean workspace-package build order, gateway wiring, web and WhatsApp, write confirmation, magic-link authentication, WhatsApp identity, and two-user scoped memory recall.

Whole-workspace strict clippy is not a clean baseline because existing `harness-core` numeric/versioning warnings fail `-D warnings`. Both the new loop crate and the agent API library pass their strict no-dependency clippy gates.

## 6. Operational use

```bash
# New main harness (also the launcher default when unset)
AELIO_HARNESS_MODE=agent_loop pnpm start

# Emergency assignment of new conversations to the old agent spine
AELIO_HARNESS_MODE=legacy pnpm start
```

Do not switch an active conversation between authorities to retry an uncertain write. Stop assigning new conversations, preserve the encrypted loop state, and let an active turn reach a safe boundary.

## 7. Final audit decision

There is no code or dependency blocker to running the new agent loop end to end in this repository. It is the default authority selected by the standard launcher when no explicit override is present, and its focused security/integration gates are green.

There are no remaining locally actionable gaps in the previously partial E05/E08/E15/E16/E21/E23/E26 scenarios, and the local `agent_loop` umbrella suite is green. The stronger statement that the entire dirty repository is production-certified is still blocked by three unrelated legacy API failures, pre-existing whole-workspace `harness-core` warning debt, and the absence of a real production canary/rollback observation window. `memory_timeline`/`memory_entity`, optional Phase 5 sub-harness/Starlark execution, and dynamic shadow comparison remain expansion work outside the flat-loop cutover scope.
