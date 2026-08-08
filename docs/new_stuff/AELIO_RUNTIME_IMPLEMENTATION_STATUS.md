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
- `[~]` Shadow/canary comparison and production readiness gates green. A live end-to-end run of the real deployment on the runtime path is green (37 checks), and upgrade/rollback plus scale drills are recorded above. Shadow comparison against the legacy path, canary rollout, and soak measurement are not done.

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
| 2026-08-08 | **finding** | **Ran a 15-minute release-build soak; it found that memory grows ~2x faster than the data and is never reclaimed** (7.15 → 15.86 KiB/row; ~3 GiB resident for 147 MiB on disk). Causes: segments are read fully into memory, compaction materialises the entire dataset twice and RSS ratchets at each cycle, and the append-only runtime tables are never pruned. Correctness was unaffected — 50,176 commits, zero errors, revision equalled turn count on every sampled subject, and the database recovered from a restart. The failing assertion is deliberately left failing. | `pnpm test:soak`; 12 checks passed, 1 failed by design |
| 2026-08-08 | **fixed** | **Closed a cross-tenant memory leak found by a security pass.** `AelioMemoryStore` read and wrote the `memories` table filtered only by `customer_id`, and the table had no tenant column at all — violating the plan's own §0.4 invariant that every operational query carries `tenant_id` as a leading predicate. Two deployments sharing one Aelio DB with default table names and a colliding subject id would recall each other's memories, and recalled memory is composed straight into a system prompt. Added migration `0004-memory-tenant-scope`, made the store tenant-scoped, and made a missing tenant throw at construction instead of emitting a malformed filter. The tenant-isolation suite now covers memory — the gap that let this pass. | Fault suite §K: a tenant recalls its own memory, another tenant recalls nothing for the same subject id, and a foreign write never appears |
| 2026-08-08 | complete | Added the upgrade/rollback drill across two real builds (`scripts/test-upgrade-rollback-drill.mjs`), the scale drill (`scripts/test-aelio-scale-drill.mjs`), and wired both into the runner. Findings are recorded below rather than asserted away. | 6 upgrade checks + 9 findings; 11 scale checks |
| 2026-08-08 | complete | Added a live end-to-end suite (`scripts/test-e2e-runtime-live.mjs`) that boots Aelio DB, the Fastify server with `sunjet.enabled: true`, and the example SDK backend, then drives the widget like a browser. This closed a real verification gap: `config.yaml` ships with `sunjet.enabled: false`, so the Phase 2–5 suite only ever exercised the legacy SQLite turn — nothing covered the runtime through the actual routes, workers, and admin surface. | 37 live checks: migration-at-boot, read-tool reply, durable Aelio DB snapshot, write confirmation + exactly-once, ambiguous-reply re-ask, authenticated/tenant-guarded HTTP ingress + duplicate rejection, operator health/ledger/redaction, cross-connection continuity. Children are spawned detached and torn down by process group — signalling only `npx` leaves its `tsx` grandchild holding the pipe and the runner never exits. |
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

## Upgrade / rollback findings (measured, not reasoned)

Run: `PREV_LL_SERVER=<previous build> node scripts/test-upgrade-rollback-drill.mjs`. The drill's
central question is whether a failure across the column-id boundary is loud or silent, because a
loud failure is survivable and a silent one is not.

| Direction | Result | Consequence |
|---|---|---|
| previous → previous (baseline) | `lossy` — dropped the colliding columns on flush | the pre-fix release was already losing data on its own; the upgrade did not cause it |
| **upgrade** (previous writes, current reads) | `lossy`, never `corrupt` | an in-place upgrade is **not** viable: the data was already gone before the new build saw it. The loss is visible (a missing field the client rejects), not silently wrong values |
| **rollback** (current writes, previous reads) | `intact` | rollback is read-safe, because column ids are persisted in the catalog file. It would still re-introduce the collision for any table it creates itself |
| rollback → write → roll forward | `intact` both cohorts | no corruption from a revert-and-recover sequence |
| newer schema, older binary | refused with an explanatory error | the migration ledger's guard works through the real boot path |

Two limits worth stating plainly: the newer-schema guard only exists from the migration-ledger
release onward, so rolling back to a build that predates it is unprotected — that build has nothing
to check against. And a database created before the column-id fix must be recreated, not upgraded.

## Scale drill findings

Run: `node scripts/test-aelio-scale-drill.mjs`. Restore integrity at ~6,000 runtime rows through
flush + compact + SIGKILL is exact: every table hashes identically, the row count is unchanged, and
sampled snapshots still read back correctly rather than merely hashing the same. Single-subject
contention behaves as designed — 40 concurrent writers produced 3 accepted commits and 37 refusals,
with the snapshot revision equal to the number of accepted writes, which is the lost-update fix
holding under real pressure.

The throughput and latency numbers the drill prints are **not** a capacity result and must not be
quoted as one: it runs on whatever machine invokes it, against the debug build, for seconds. A soak
still needs hours on representative hardware.

## Soak findings (15 minutes, release build, 2026-08-08)

Run: `pnpm test:soak` (`AELIO_SOAK_MINUTES` to lengthen). 50,176 commits at ~56/s, **zero errors**,
and correctness held throughout: `revision` equalled the turn count on every sampled subject, no
effect ended `unknown` or `failed`, and the database recovered from a restart afterwards.

| Metric | Start | End | Change |
|---|---|---|---|
| RSS | 377 MiB | 2,854 MiB | +657% |
| On-disk dataset | 18 MiB | 147 MiB | +712% |
| **Memory per stored row** | **7.15 KiB** | **15.86 KiB** | **+122%** |
| p95 latency | 159 ms | 329 ms | +107% |

**The suite fails one check, and it is left failing.** Memory per row more than doubling means RSS
grows about twice as fast as the data — roughly 3 GiB resident for 147 MiB on disk. Relaxing the
threshold would have made it green and hidden a real defect.

Two compounding causes, both visible in the sample curve:

1. **Everything is resident.** `ll_format::read_file` does `fs::read`, so every segment is held
   fully in memory. Nothing is memory-mapped or paged.
2. **Compaction materialises the whole dataset twice, and RSS ratchets.** `Database::compact`
   builds a `Vec<(u64, Row)>` of every live row and then *clones* each into a synthetic memtable.
   The jumps line up exactly: the 13:00:35 sample caught a compaction in flight (8 files, disk
   momentarily 273 MiB against a 145 MiB steady state) and RSS rose 2,006 → 3,077 MiB in that one
   window and never came back down.

3. **Nothing is ever pruned.** `runtime_events`, `runtime_ledger`, and `runtime_outbox` are
   append-only and no code deletes from them — confirmed by search, not assumption. So the dataset
   grows with lifetime event volume regardless of how small the active subject set is.

Together these mean a long-running deployment grows RSS until it is out of memory, and it will hit
that wall sooner than the disk figures suggest. Throughput and latency degraded far more gently
(p95 roughly doubled and then held), so **memory is the binding constraint, not speed.**

## Remaining hard gates before a production claim or the requested final commit/push

- `[x]` Runtime outbox retry has durable availability, lease expiry/fencing, error capture, exponential backoff, and terminal attempt limits. Meta WhatsApp still offers no provider-side idempotency key — that is a property of Meta's API, not something Aelio can fix — so an ambiguous send is now parked as `unknown` for reconciliation instead of being resent. This trades a possible missed message for a guaranteed absence of duplicate customer messages, which is the correct default; a tenant wanting the opposite must opt in explicitly.
- `[x]` Prompt, memory, tool, and spawn nodes run behind per-node instance commits plus an append-only intent/result journal, and the same journal now guards harness tool calls inside a turn. A journal entry stuck in `unknown` stops the instance and reports, which is safe but still requires an operator to resolve — an automatic reconcile workflow per tool is not built.
- `[!]` The server still boots the legacy SQLite/Drizzle services and its SDK, auth, legacy worker, migration, and data-retention routes still depend on them. Aelio DB is authoritative only on the cut-over runtime paths, not yet the sole platform store.
- `[~]` Two-adjacent-release upgrade/rollback is now measured against real builds; the findings and their two limits are recorded above.
- `[~]` Restore/replay is verified at ~6,000 rows through flush + compact + SIGKILL. A production-sized dataset on production hardware is still untested.
- `[~]` A security pass has run over the branch and found one real cross-tenant leak, now fixed and covered. This is not an independent review — the same person wrote and reviewed the code, which is exactly the weakness an external review exists to cover.
- `[!]` **Memory scales with lifetime data, roughly 2x faster than the data itself, and is never reclaimed.** Measured, not theorised — see the soak findings above. Fixing it needs three things: streaming compaction instead of a full in-memory materialisation, segment paging or mmap instead of `fs::read`, and a retention/pruning policy for the append-only runtime tables. This is the single largest blocker to running Aelio DB as a long-lived production store, and it is why the soak suite is left with a failing check.
- `[!]` Still not run: sustained multi-replica load, a soak measured in hours or days rather than minutes, and the production canary period. These genuinely need representative hardware, real traffic, and elapsed time.
- `[!]` SQLite is deliberately still resident. §0.5 of the plan gates its deletion on tenant-by-tenant cutover, verified export hashes, restore drills, and an expired retention window — removing it now would violate the migration plan it belongs to, not complete it.
- `[!]` The column-id fix changes what a column id means on disk. A database created before it must be recreated, not upgraded — there is no in-place migration, and its flushed segments already lost the mistyped columns. This is safe today only because Aelio DB is not yet the authoritative platform store.
- `[!]` Do not present this as a certified production cutover. It is a complete runtime verified by roughly 200 checks — against a real Aelio DB (fault injection, migration, isolation, restore, scale, upgrade/rollback) and against a live server, SDK, and widget on the runtime path — with the legacy SQLite platform still underneath by design.
