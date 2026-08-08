# Aelio Stateful Runtime — Implementation Status

**Authority:** [AELIO_STATEFUL_AGENT_RUNTIME_IMPLEMENTATION_PLAN.md](AELIO_STATEFUL_AGENT_RUNTIME_IMPLEMENTATION_PLAN.md)  
**Started:** 2026-08-07  
**Target:** Aelio DB is the only durable runtime, memory, retrieval, artifact, ledger, outbox, and scheduler store. SQLite/Drizzle and the Sunjet compatibility name are migration-only and are removed from the target topology.

## Status legend

- `[x]` implemented and verified in this repository
- `[~]` implementation in progress or present but not yet verified against its acceptance gate
- `[ ]` not started
- `[!]` blocked by a prerequisite or requires an explicit design decision

## Current baseline audit

- `[x]` Existing TypeScript harness provides bounded planner, schema-derived dependency resolution, tool gates, read parallelism, serialized writes, budgets, and a basic suspended-plan ledger.
- `[x]` Current Astrolobe engine provides WAL durability, MVCC, crash recovery, typed catalog, mutations, compaction, and scalar/vector/text/graph querying.
- `[x]` Storage/runtimes architecture and implementation plan written and updated for Aelio DB as sole authority.
- `[x]` Prompt Forge/Resolver/Composer architecture added to the plan.
- `[~]` Aelio DB now has atomic multi-mutation WAL transactions through the engine, query facade, HTTP API, and TypeScript client. Conditional writes, logical keys, uniqueness, leases/fencing, idempotency, and an outbox dispatcher remain required before it can safely own effects.
- `[!]` `aelio-os/` is an untracked build-artifact directory without source/Cargo manifest. The active storage source is `Sunjet/Astrolobe`; no edits will be made to the artifact-only directory.

## Phase S — Aelio DB transactional foundation and naming migration

- `[x]` Create this implementation status ledger and lock Aelio DB naming.
- `[x]` Add multi-mutation transaction batches to `ll-engine` and `ll-query::Database`.
- `[~]` Add client-chosen logical keys, exact key lookup, row revision/LSN reads, and conditional mutation/CAS. Conditional batches now support an `absent` delivery-key guard and `row_matches` value CAS; catalog-enforced logical keys and exposed row revision tokens remain to be implemented.
- `[ ]` Add catalog-enforced unique constraints and insert-if-absent semantics.
- `[x]` Add durable leases with expiry and fencing tokens. Implemented as CAS lease/renew/expire over the outbox and scheduled-event tables (`lease_token` + `lease_expires_at` + `revision` fencing); verified for both single-winner leasing and expired-lease requeue.
- `[x]` Add transaction idempotency records. Ingress dedupes on `(tenant_id, idempotency_key)` inside the commit transaction; artifact effects use an append-only intent/result journal that distinguishes `new`, `completed`, and `unknown`.
- `[x]` Expose atomic transaction batches through `ll-server` and the existing TypeScript client compatibility package.
- `[x]` Add Aelio DB runtime table catalog and schema migration/version API. `migrateAelioStorage` boots through a versioned ledger under a CAS lease: migrations apply at most once, replicas starting together serialize, an edited already-applied migration is refused, an older binary refuses to start against a newer schema, and destructive steps are refused unless explicitly enabled.
- `[ ]` Add checkpoint/export/restore/integrity operations and metrics/readiness.
- `[ ]` Rename public server/client/config integration from Sunjet/Astrolobe to Aelio DB / `@aelio/storage`, retaining a bounded compatibility alias.
- `[~]` Verify model-based crash/recovery, transaction atomicity, CAS contention, leases, idempotency, and outbox tests. Atomicity, CAS contention, leases, idempotency, and outbox retry are covered by `scripts/test-aelio-runtime.mjs` (66 checks) and the Rust suites. Randomized multi-seed crash-at-every-WAL-boundary simulation against a reference model is still outstanding.

## Runtime control plane

- `[x]` Add `RuntimeEventV1`, `RuntimeSnapshotV1`, and `RuntimeDecisionV1` protocol contracts.
- `[x]` Build event inbox, subject actors, runtime ledger, outbox, and scheduled-event repositories over Aelio DB. `AelioRuntimeStore` atomically commits an event claim, subject snapshot CAS revision, immutable ledger records, and pending effects, plus durable instance CAS transitions, one-time continuation consumption, scheduled-event lease/requeue/complete, and outbox lease/retry/expiry recovery with exponential backoff. Verified end to end by `scripts/test-aelio-runtime.mjs`.
- `[x]` Replace the reply-only conductor with the real Aelio control plane (`packages/core/src/runtime/aelio-conductor.ts`). A runtime turn now runs lifecycle-scoped tool filtering, relevance retrieval, memory recall, the full plan/bind/resolve/execute harness, confirmation gating, and durable park/resume — none of it touching SQLite.
- `[x]` Make parked plans a first-class Aelio DB record (`AelioSuspensionStore` over `runtime_continuations`), so a mid-plan confirmation survives a process restart and resumes the whole plan rather than only the pending call.
- `[x]` Apply declarative lifecycle transitions into the durable snapshot on tool success, via an explicit `onToolSuccess` harness hook instead of the Drizzle writer.
- `[x]` Make artifact execution durable step-by-step: `stepRuntimeArtifact` executes exactly one approved node and the runner commits cursor + ledger + effects after each, so a crash resumes at the next node.
- `[x]` Implement the approved artifact node adapters. `memory` is instance-scoped and commits with its step; `tool` and `prompt` run under an intent/result/unknown effect journal keyed by instance + node + argument hash; `spawn` supports bounded inline children and detached child instances with derived (non-random) ids.
- `[x]` Add pinned prompt artifacts with declared slots, JSON-encoded slot rendering (a customer string cannot terminate the template), and a fail-closed missing-slot check.
- `[~]` Implement tenant state catalog admission, workflow artifacts declared by the SDK, child join/await, cancellation, and expiry sweeps. Child spawn + `join` is implemented end to end: a detached child is created and queued from the parent node (derived ids, so a retried parent joins rather than forks), the parent parks durably, and each child's completion commits a `harness.resume` wake in the same transaction that marks it complete. A join naming a non-spawn node is rejected at admission. SDK-declared workflow artifacts, cancellation, and expiry sweeps remain.
- `[~]` Converge pending confirmation and plan suspension into the unified continuation/effect state machine. The runtime path is converged onto one parked-plan record; the legacy SQLite `pending_confirmations` table is still read by `processTurn` for tenants that have not cut over.
- `[x]` Move harness planning/execution under the runtime, committing its outcome in the same conditional transaction as the event claim and ledger.
- `[x]` Replace the interval daemon with leased scheduled-event processing for runtime ingress (Web, WhatsApp, SDK). The legacy reflection daemon still runs for the SQLite path.

## Memory, retrieval, and prompts

- `[ ]` Implement `@aelio/memory` typed observation, merge, retention, canonicalization, and embedding APIs on Aelio DB.
- `[ ]` Add Aelio DB multimodal catalog cards for tools, workflows, artifacts, memory, and evidence.
- `[ ]` Add behavioral overlay rules and durable counters.
- `[ ]` Add prompt artifact catalog, resolver, deterministic composer, output validator, and redacted prompt ledger.
- `[ ]` Add Prompt Forge candidate/sandbox/evaluation/promotion workflow and signed vendor prompt bundle.

## Edge, SDK, migration, and operations

- `[ ]` Update SDK catalog with versioned state/workflow/tool output/retry/idempotency contracts.
- `[x]` Change Web, WhatsApp, and SDK ingress to normalize/enqueue runtime events. `POST /v1/runtime/events` and the Web widget run the Aelio Runtime when Aelio DB is configured; WhatsApp and SDK ingest both persist a scheduled Aelio DB inbox event and acknowledge before any conductor/LLM work, with the leased scheduler and the Aelio outbox doing the rest. The widget no longer fabricates a reply on a duplicate or conflicting turn.
- `[ ]` Migrate existing SQLite data through verified export/import and tenant-level cutover.
- `[ ]` Replace SQLite/Drizzle runtime reads/writes and delete legacy dual authority after retention gates.
- `[~]` Add replay inspector, metrics, alerting, operational runbooks, backups, restore drills, and deployment migration. The operator surface exists (`/api/v1/admin/runtime/*`: subject view, turn ledger replay, queue health with a `degraded` signal, audited cancel, unknown-effect reconciliation) — all authenticated, tenant-scoped, and payload-redacted. A restore drill runs in CI. Metrics export, alert wiring, and runbooks remain.

## Verification gates

- `[x]` Storage transaction and WAL fault-injection suite green. `scripts/test-aelio-runtime-faults.mjs` crashes the process at every storage boundary of a confirmed write and asserts exactly one side effect after recovery, plus a SIGKILL Aelio DB restart that must recover state, ledger, outbox, and idempotency through the WAL alone. The Astrolobe suite includes a regression test that a batch is invisible at every snapshot before its commit LSN.
- `[x]` No duplicate effect across retries/crashes/provider redelivery. Every crash boundary of a confirmed write recovers to exactly one tool invocation; a five-way provider redelivery storm yields one claim and one queued reply; an artifact never replays a completed node. A send whose outcome cannot be verified is parked as `unknown` rather than repeated.
- `[~]` Nested workflow park/resume, cancellation, and catalog upgrade suite green. Parent/child park→wake→join is covered against the real runner. Cancellation and catalog upgrade of a live instance are not.
- `[x]` Cross-channel subject continuity suite green. One subject over web, WhatsApp, and SDK shares a single durable state and turn counter, while each channel keeps its own delivery path.
- `[ ]` Prompt artifact security/schema/evaluation suite green.
- `[x]` Aelio DB restore/replay and tenant-isolation suite green. The restore drill flushes to segments, hard-restarts, and compares a content hash of every runtime row; tenant isolation is asserted across snapshots, instances, ledger, continuations, parked plans, and idempotency keys using a deliberately shared subject id. Compaction is exercised by the Rust suite but not yet by the runtime drill.
- `[ ]` Shadow/canary comparison and production readiness gates green.

## Change log

| Date | Status | Change | Verification |
|---|---|---|---|
| 2026-08-07 | complete | Created implementation ledger and recorded source/worktree constraints. | Manual repository audit |
| 2026-08-07 | in progress | Began Phase S by mapping `Sunjet/Astrolobe` engine and server transaction boundaries. | Source audit; implementation pending |
| 2026-08-07 | complete | Added atomic ordered insert/update/delete batches to the WAL engine and `Database`, with a single durable commit point and recovery coverage. | `cargo test -p ll-engine -p ll-query` |
| 2026-08-07 | complete | Added authenticated `POST /v1/transactions`, runtime-oriented API coverage, and `SunjetClient.transact` compatibility method. | `cargo test -p ll-server -p ll-query -p ll-engine`; `pnpm --filter @aelio/sunjet-client typecheck` |
| 2026-08-07 | complete | Added conditional Aelio DB transactions: atomic absent-key event claims and row-value CAS. A failed condition appends no WAL mutation. | `cargo test -p ll-query --test mutations`; `cargo test -p ll-server --test api` |
| 2026-08-07 | removed | Removed the unadopted SOL interpreter/catalog after confirming it was not part of the intended architecture. It was isolated and was never in a production route. | Source audit |
| 2026-08-07 | complete | Added additive Aelio runtime storage catalog/config bootstrap for inbox, snapshots, ledger, outbox, schedules, workflows, and prompts. | Source audit; clean `git diff --check` |
| 2026-08-07 | in progress | Added typed runtime contracts, transactional Aelio runtime store, constrained conductor entry point, approved-node executor, and initial built-in math artifacts. | Isolated strict TypeScript check and artifact execution smoke test |
| 2026-08-07 | complete | Repaired offline workspace links, rebuilt package dependencies, and fixed stale telemetry/protocol type drift so core and server compile together. | `pnpm --filter @aelio/core typecheck/build`; `pnpm --filter @aelio/server typecheck/build` |
| 2026-08-07 | complete | Added outbox CAS claim/completion, one-time continuation CAS consumption, built-in artifact installation at Aelio DB boot, and SQLite-independent runtime ingress. | Real local Aelio DB integration: transactional event/snapshot/ledger/outbox, duplicate rejection, outbox claim/delivery, continuation single-consume |
| 2026-08-07 | in progress | Cut Web widget and configured WhatsApp ingress to the Aelio Runtime path; added an Aelio DB-only outbox dispatcher for replies and continuations. | `pnpm --filter @aelio/core typecheck/build`; `pnpm --filter @aelio/server typecheck/build` |
| 2026-08-08 | in progress | Added durable pinned workflow instances, instance transition CAS, scheduled-event leases/recovery, a scheduler worker, and WhatsApp asynchronous durable inboxing. | Real local Aelio DB test: schedule → lease → complete and expired lease → requeue; `pnpm --filter @aelio/core typecheck/build`; `pnpm --filter @aelio/server typecheck/build` |
| 2026-08-08 | complete | Made Aelio DB bootstrap validate existing table schemas and fail closed on missing/incompatible runtime columns. | `pnpm --filter @aelio/sunjet-client typecheck/build`; core/server strict typechecks |
| 2026-08-08 | complete | Added persisted additive catalog columns through Astrolobe, its HTTP API, and the TypeScript client; upgraded runtime outbox rows to leased, fenced, retryable delivery records. | Catalog/query/server focused tests (4 + 11 + 14); real Aelio DB retry and expired-lease recovery test; core/server builds |
| 2026-08-08 | **fixed** | **Repaired an MVCC batch-atomicity regression introduced with `Engine::transact`.** Rows were stamped with their own WAL record LSN while `transact` returned the commit LSN, so a reader whose snapshot fell between two records of one transaction could observe half a batch, and the returned LSN was unusable as a read-your-writes snapshot. Rows in a batch now become visible at one shared commit LSN, in the live memtable and on recovery. | `cargo test` across the whole Astrolobe workspace (was 1 failing: `ll-query --test file_source`); new regression test `a_batch_is_invisible_at_every_snapshot_before_its_commit` |
| 2026-08-08 | complete | Replaced the reply-only runtime conductor with the real Aelio control plane: lifecycle-scoped tools, relevance retrieval, memory recall, the full harness, confirmation gating, and Aelio DB-durable park/resume. Added `AelioSuspensionStore` and `AelioMemoryStore`, and an `onToolSuccess` hook so lifecycle transitions land in the durable snapshot instead of SQLite. | `scripts/test-aelio-runtime.mjs` against a real Aelio DB: 66 checks incl. read-tool execution, park → restart → resume-once, ambiguous-reply re-ask, denial, lifecycle transition |
| 2026-08-08 | complete | Made artifact execution durable per node (`stepRuntimeArtifact` + per-step instance commit) and enabled the tool/prompt/memory/spawn adapters behind an intent/result/unknown effect journal. Added pinned prompt artifacts with declared slots and JSON-encoded rendering. | `scripts/test-aelio-runtime.mjs` artifact + effect-journal sections; core/server strict typechecks and builds |
| 2026-08-08 | **fixed** | **Repaired silent data loss on flush.** Column ids were assigned `1..=n` *per table*, but every table shares one memtable and flushes into one segment keyed by column id alone — so `runtime_snapshots.revision` (id 4, i64) collided with `runtime_events.idempotency_key` (id 4, utf8), the flush typed the id from whichever row it saw first, and every value of the other type was dropped. Any subject snapshot became unreadable after the first flush. Column ids now come from a global space partitioned per table. | New restore drill (§L) caught it; `cargo test` across the workspace; new `column_ids_are_unique_across_tables` and column-slice-overflow regression tests |
| 2026-08-08 | complete | Added the versioned schema migration ledger with a CAS lease, at-most-once application, checksum guard against edited migrations, fail-closed on an unknown newer schema, and refusal of destructive steps unless explicitly enabled. | Fault suite §I (13 checks) |
| 2026-08-08 | complete | Added the operator/replay surface: subject view, turn ledger reconstruction, queue health with a `degraded` signal, audited instance cancel, and unknown-effect reconciliation. Authenticated, tenant-scoped, payload-redacted. | `pnpm typecheck`; server build |
| 2026-08-08 | complete | Added cross-channel continuity, tenant isolation, and restore-drill coverage. | Fault suite §J/§K/§L; 70 fault checks total |
| 2026-08-08 | complete | Cut SDK ingest onto the durable Aelio DB inbox (persist-then-ack, leased scheduler drains it), and stopped the Web widget answering a duplicate/conflicting turn with a fabricated reply. | `pnpm typecheck` (15 tasks); `pnpm build`; runtime suite |
| 2026-08-08 | complete | Authenticated `POST /v1/runtime/events` with the shared secret and rejected a body `tenant_id` other than this deployment's — the endpoint runs model and tool work on a caller-named subject and was previously unauthenticated. | Source review; `pnpm --filter @aelio/server typecheck` |
| 2026-08-08 | **fixed** | **Closed a lost-update flaw the new fault suite exposed.** `commit` re-read the snapshot and CAS'd against the *latest* revision, so a decision computed from an older revision was silently rebased onto state it had never seen. Commits now assert the revision the decision was made on; `runConductorEvent` passes it, and a stale decision is refused rather than applied. | Fault suite section B: 6 concurrent turns for one subject → 1 commit, 5 conflicts, snapshot advances by exactly 1, no lost history |
| 2026-08-08 | **fixed** | **Closed an exactly-once hole the fault suite exposed.** A tool runs before the turn commits, so a crash in between re-ran the write when the provider redelivered. Tool invocation is now wrapped in the durable intent/result journal, keyed by the event's idempotency key plus tool and canonical arguments — stable across replays of one logical event, distinct for a genuinely new message. An interrupted call resolves to `unknown` and fails closed. | Fault suite section A: crash at all 6 storage boundaries of a park+confirm write, each recovering to exactly one invocation |
| 2026-08-08 | complete | Implemented detached child spawn and `join`: children are created and queued from the parent node with derived ids, the parent parks durably, and a child's completion commits its parent wake in the same transaction. Added join admission validation. | Fault suite section G, driving the real `runtime-artifact-runner` through the durable outbox |
| 2026-08-08 | complete | Added the effect reconciliation policy: an expired dispatch lease is only requeued when redelivery is provably deduplicated downstream (internal effects, web, SDK channels carrying `aelio_effect_id`). Meta WhatsApp is not, so its ambiguous sends park as `unknown` with `listUnknownEffects`/`resolveUnknownEffect` for reconciliation. | Fault suite sections D and E |
| 2026-08-08 | complete | Added a bounded conflict retry to the runtime entry point, safe only because tool calls are journaled. | `pnpm typecheck`; full suite |
| 2026-08-08 | complete | Restored flow-progress parity on the runtime path: durable `flowProgress` in the subject snapshot, the active flow step's tool force-included past relevance filtering, and SDK `set_state`/`set_flow_progress` pushes patched into the Aelio DB snapshot (CAS with retry) as well as SQLite. | `scripts/test-aelio-runtime.mjs` SDK-push section; 71 checks total |
| 2026-08-08 | complete | Fixed two blockers in the existing suite (a duplicated `ws` import in the Phase 5 script, and `run-tests.mjs` colliding with a developer's own server on the default port). | Full `node scripts/run-tests.mjs`: LLM wiring, harness, Aelio runtime (71), Phases 2/3/4/4/4/5 all green |

## Remaining hard gates before a production claim or the requested final commit/push

- `[x]` Runtime outbox retry has durable availability, lease expiry/fencing, error capture, exponential backoff, and terminal attempt limits. Meta WhatsApp still offers no provider-side idempotency key — that is a property of Meta's API, not something Aelio can fix — so an ambiguous send is now parked as `unknown` for reconciliation instead of being resent. This trades a possible missed message for a guaranteed absence of duplicate customer messages, which is the correct default; a tenant wanting the opposite must opt in explicitly.
- `[x]` Prompt, memory, tool, and spawn nodes run behind per-node instance commits plus an append-only intent/result journal, and the same journal now guards harness tool calls inside a turn. A journal entry stuck in `unknown` stops the instance and reports, which is safe but still requires an operator to resolve — an automatic reconcile workflow per tool is not built.
- `[!]` The server still boots the legacy SQLite/Drizzle services and its SDK, auth, legacy worker, migration, and data-retention routes still depend on them. Aelio DB is authoritative only on the cut-over runtime paths, not yet the sole platform store.
- `[!]` Still not run: sustained multi-replica load and soak measurement, restore drills against a production-sized dataset, two-adjacent-release upgrade/rollback, a security review, and the production canary period. These are the §0.6 gates that need an environment and a clock, not more code.
- `[!]` SQLite is deliberately still resident. §0.5 of the plan gates its deletion on tenant-by-tenant cutover, verified export hashes, restore drills, and an expired retention window — removing it now would violate the migration plan it belongs to, not complete it.
- `[!]` The column-id fix changes what a column id means on disk. A database created before it must be recreated, not upgraded — there is no in-place migration, and its flushed segments already lost the mistyped columns. This is safe today only because Aelio DB is not yet the authoritative platform store.
- `[!]` Do not present this as a certified production cutover. It is a complete runtime, verified by 141 end-to-end checks including fault injection, migration, isolation, and restore, with the legacy SQLite platform still underneath by design.
