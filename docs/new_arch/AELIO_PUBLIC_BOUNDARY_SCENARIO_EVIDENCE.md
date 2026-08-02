# Aelio public-boundary scenario evidence

**Audit date:** 2026-08-01  
**Rule:** `covered` means the complete scenario and every required assertion in Unified Master §10
runs through one public Rust boundary. Component tests are useful evidence, but do not make a row
covered. This prevents the release checklist from converting many green unit tests into an
unsupported end-to-end claim.

| # | Mandatory scenario | Current executable evidence | Public status | Missing closure |
|---|---|---|---|---|
| 1 | cold/warm greeting | `admitted_bound_procedure_executes_through_unified_runtime_and_replays` drives five public cold Tier-2/model-free turns, observes durable promotion, binds an admitted pure artifact, then proves public Tier-0 warm execution, zero effects/state mutation, exact artifact ledger/version, decision explanation and replay | covered | — |
| 2 | invalid phone, OTP send, wrong/correct OTP, restart parked | `materialized_public_flow_owns_effect_and_continuation_end_to_end` proves every branch through `/v1/turns`, zero/one effect counts, exact artifact version, active/closed subject index, metadata-only decision trace, strict `call_intent → call_dispatch → call_result` ledger order, database reopen, and post-completion replay without resend | covered | — |
| 3 | duplicate input and duplicate tool result | `public_sdk_socket_acks_and_discards_duplicate_tool_results` drives `/v1/turns` and the authenticated `/v1/sdk` socket together, proves one version-pinned invocation, strict `call_intent → call_dispatch → call_result`, accepted-then-duplicate classification, a redacted durable disposition audit, identical turn replay, and no second SDK effect | covered | — |
| 4 | SDK disconnected before/after dispatch | `public_sdk_disconnect_distinguishes_pre_dispatch_from_unknown_outcome` proves an idle disconnect is rejected by `/v1/turns` before any durable effect intent, while a disconnect after the version-pinned WebSocket write returns fail-loud manual review, retains exact `call_intent → call_dispatch` evidence with no fabricated result, and exposes only hashed identities in the durable admin outcome view | covered | — |
| 5 | read-only detour and deferred second flow | `materialized_public_flow_owns_effect_and_continuation_end_to_end` asks a declared read-only boundary question while login is parked, executes its exact admitted pure artifact, proves zero added effects plus byte-identical continuation/version and replay, then submits a second flow intent, proves acknowledgement/defer with no execution, persists one redacted `open_loop` memory, replays identically, reopens both databases, resumes the original login continuation, and on completion durably advances the oldest deferred intent to `ready` while returning its redacted text to the user | covered | — |
| 6 | Prism scalar/text/vector/graph/fusion | `unified_authenticated_prism_covers_modalities_and_rejects_invalid_envelopes` composes the production runtime and database routers, authenticates one public `/v1/prism` boundary, executes scalar/text/vector/graph/fused plans, verifies mandatory projection/no field leakage, emits a metadata-only `aelio-prism` decision narrative with accepted modalities/bounds, proves deterministic replay, and verifies source rows remain unchanged | covered | — |
| 7 | invalid Prism inputs | the same unified public scenario proves unauthenticated denial plus fail-closed unknown-field, over-limit, vector-dimension, and graph-budget responses, with no result envelope or row mutation; the deeper parser/engine adversarial suite remains defense-in-depth | covered | — |
| 8 | demand → build → sandbox canary | `public_demand_builds_and_sandbox_gates_a_later_canary` authenticates and de-duplicates a capability request, proves the body cannot forge its requester, queues the off-path build, advances its bounded persisted state machine, supplies closed-schema select/compose reactions, assembles and sandbox-gates a pure harness as canary, resolves the original demand, then publicly executes and exactly replays the later admitted pin | covered | — |
| 9 | reviewed approval before consumption | `reviewed_artifact_is_inert_until_public_deployer_approval` pushes a flow with reachable external effect, runs its complete shadow gate without an identified deployer, proves public consumption is denied, then repeats the gate through an authenticated deployer principal, verifies the append-only actor record, and only then consumes the canary | covered | — |
| 10 | canary promotion and Guard demotion | `public_canary_evidence_promotes_and_guard_demotes_immediately` ingests 20 distinct successful observations through the authenticated control plane, proves duplicate evidence cannot inflate the threshold, applies and idempotently re-applies the persisted promotion proposal, consumes the promoted pin, then submits one attributed Guard violation and proves the atomic system-authored `promoted → shadow` transition plus immediate denial of later consumption | covered | — |
| 11 | dependency version changes | `public_dependency_and_kernel_migrations_apply_exact_conservative_cascades` drives authenticated prompt/model/embedding/imprint departures, proves exact transitive demotion plus one rebuild demand per affected artifact, derives the running kernel version inside Rust, returns only mismatched machine-built promoted artifacts to canary, refuses execution of any stale kernel pin, and proves migration retry is empty/idempotent | covered | — |
| 12 | cross-tenant multimodal attacks | `tenant_credentials_isolate_rows_modalities_artifacts_and_evidence` runs two credentials bound to two tenants against one composed server/database process, gives both the same logical collection name, and proves physical isolation for row ids plus scalar/text/vector/graph/fused Prism; it also proves tenant B cannot execute tenant A's artifact or inject its lifecycle evidence while tenant A remains functional | covered | — |
| 13 | replay after database changes | `historical_replay_injects_ledgered_read_after_database_change` executes a host-backed database read through a public Flow, mutates the real source row, calls authenticated `/v1/replay`, proves the committed/recomputed hashes match and the read-call count does not increase, then proves a fresh instance sees the changed row | covered | — |
| 14 | late result after unknown outcome | `public_sdk_disconnect_distinguishes_pre_dispatch_from_unknown_outcome` reconnects after fail-loud unknown outcome, submits the original correlation, receives `late`, proves exact `call_intent → call_dispatch → late_result`, verifies the redacted durable late audit, and proves the durable tool state remains `manual_review` rather than re-entering the turn | covered | — |
| 15 | graceful drain | `sigterm_drains_queued_turns_and_restart_recovers_active_build` launches the real binary, persists an active build stage, queues two identical turns behind one slow read, sends real SIGTERM, proves graceful completion with one effect, restarts on the same data, replays the completed turn without another read, and resumes the build from its exact persisted stage | covered | — |

## Newly closed seam

The catalog-to-runtime continuation seam is now real and public-testable:

```text
POST /agent/v1/catalog with flow_artifacts[flow_id] = exact admitted pin
  → deterministic authored-flow activation
  → aelio-runtime writes subject-hashed continuation index before execution
  → kernel Park (legacy active_flow remains null)
  → next POST /agent/v1/turns resumes the exact runtime instance
  → completion closes the subject index
  → duplicate public turn id replays the durable response without a second wake
```

The relevant acceptance tests are:

- `bound_authored_flow_parks_and_resumes_only_in_runtime_continuation`
- `subject_continuation_owns_park_restart_resume_and_reactivation`
- `admitted_bound_procedure_executes_through_unified_runtime_and_replays`
- `unmaterialized_public_flow_fails_closed_before_a_runtime_proxy_effect`
- `materialized_public_flow_owns_effect_and_continuation_end_to_end`
- `public_sdk_socket_acks_and_discards_duplicate_tool_results`
- `sdk_delivery_dispositions_are_redacted_and_survive_restart`
- `public_sdk_disconnect_distinguishes_pre_dispatch_from_unknown_outcome`
- `public_demand_builds_and_sandbox_gates_a_later_canary`
- `reviewed_artifact_is_inert_until_public_deployer_approval`
- `public_canary_evidence_promotes_and_guard_demotes_immediately`
- `public_dependency_and_kernel_migrations_apply_exact_conservative_cascades`
- `tenant_credentials_isolate_rows_modalities_artifacts_and_evidence`
- `historical_replay_injects_ledgered_read_after_database_change`
- `sigterm_drains_queued_turns_and_restart_recovers_active_build`

Unified production now disables the legacy FlowSpec executor. An unbound authored flow emits a
deduplicated off-path materialization demand and no effect; readiness reports the missing binding.
Reviewed learned flows likewise remain `Approved` until an admitted executable artifact is
verified and atomically published with their catalog activation. The legacy interpreter exists
only in explicitly constructed local/parity worlds.

The SDK bridge now delegates delivery state to `aelio-wire::DeliveryBook`. A successful WebSocket
write marks dispatch, reconnect requeues unresolved calls with the same correlation, the first
valid result is the only result delivered to the waiting turn, and later duplicates or late
results receive an explicit disposition instead of mutating execution. Accepted, duplicate, and
late dispositions are persisted as SHA-256 correlation digests; raw socket correlations are not
stored or returned by the admin audit boundary. Result acknowledgements include the retained
metadata-only delivery ledger, making `late_result` ordering auditable without exposing arguments
or outputs.

External host installation also remains inside `DurableRuntime`: SDK and runtime-proxy adapters
are wrapped by the same durable idempotency lease/outcome host instead of replacing it. A socket
loss before dispatch therefore creates no outcome record; a dispatched non-idempotent effect with
no result is persisted as `manual_review` and fails loudly. The bounded admin tool-outcome view
hashes both subject and idempotency identities and omits result payloads and failure free text.

Runtime continuation probing is now separate from waking. The flow gate may execute only a bound,
promoted, non-instantiating procedure whose contract, path, and every dependency are read-only;
that exact artifact runs without touching the parked subject continuation. A second authored-flow
intent is acknowledged and stored as a redacted `open_loop` memory instead of creating a flow
stack. Duplicate detour/defer turns replay without another artifact call, memory write, or effect.
When the original runtime continuation closes, the oldest deferred intent advances atomically to
`ready` and is surfaced in the completion reply instead of remaining a hidden permanent reminder.

Prism's production HTTP response now carries a metadata-only decision narrative: authority,
collection, accepted modalities, predicate count, explicit projection, limit, and result count.
It never echoes query text, vectors, graph seeds, or row values. The unified server test exercises
all modalities and invalid envelopes behind the same bearer-authenticated route composition used
by `aelio-server` and proves deterministic read replay with no storage mutation.

The public capability lifecycle now has an executable off-path proof. Requester identity is derived
from the bearer credential, equivalent needs coalesce by hash, and the requesting turn does not
synchronously invent or execute code. A queued build progresses through persisted bounded stages
and explicit closed-schema oracle reactions; its assembled harness must pass the declared sandbox
cases before the demand becomes resolved and the artifact is executable as canary. Completed
runtime instances also retain the terminal input hash, so an identical network retry returns the
verified stored result while a different input under the same instance id remains a conflict.

Reviewed reach remains inert after shadow validation until an authenticated deployer is written
into the lifecycle history. Canary evidence is de-duplicated by input identity, promotion is a
persisted proposal applied by the lifecycle authority, and proposal application is retry-safe.
Attributed observations now apply to both canary and promoted artifacts: any Guard violation can
therefore atomically demote a promoted pin to shadow, after which the execution boundary refuses it.

Dependency departure now treats interface imprints as first-class pins alongside prompt, model,
embedding, target, artifact, converter and dataset pins. The authenticated migration boundary emits
deterministic cascades and rebuild demand. Kernel migration derives the running version rather than
accepting it from a request, conservatively returns mismatched learned artifacts to canary, and the
executor refuses every stale kernel pin until explicit migration resolves it.

Tenant-scoped credentials are available at both Rust control-plane and database boundaries. The
database maps logical collections into credential-derived physical namespaces before schema, row,
text, vector, graph, fusion, NL and explain operations; the runtime independently checks every body
or query tenant against its credential. Historical replay is exposed as a metadata-only report over
the exact immutable Flow and durable hash-chained ledger; its replay backend injects recorded reads
and effects and cannot call the live host. Real SIGTERM coverage verifies Axum admission shutdown,
in-flight/queued HTTP drain, durable turn replay and persisted build resumption across process restart.

## Release interpretation

All fifteen rows now have executable public-boundary coverage. `E2E-PUBLIC-001` may move from
`partial` only after the complete release gate below passes and the requirement index points to
these exact tests. Local deterministic coverage does not replace the external live-provider,
channel, load, soak and crash-injection matrix tracked by `PRODUCTION-LIVE-001`.
