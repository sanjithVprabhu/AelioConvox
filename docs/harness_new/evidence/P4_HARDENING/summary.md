# P4 agent-loop gap-closure evidence

**Date:** 2026-08-10  
**Scope:** All locally actionable gaps previously marked Partial in E05, E08, E15, E16, E21, E23, and E26. Sol excluded.

## Closed gaps

| Scenario | New evidence |
|---|---|
| Missing arguments | Whole batch parks before policy/rate/SDK, `tool_calls=0`, encrypted wait state resumes after restart |
| Confirmation tamper | Canonical digest changes when parked arguments change |
| Provider overload | Scripted 429 and 503 retry to success under one request ID and distinct attempt IDs |
| Context overflow | Typed overflow performs one tagged compaction and one retry without charging the failed call |
| Crash after dispatch | Persistent turn/batch/ordinal reservation ignores fresh provider call IDs and rejects changed replay |
| Lifecycle login | Successful send grants only verify continuation; verified evidence applies idempotent authenticated transition; protected read then succeeds |
| Cancellation | Fence-bound marker reaches a concurrent turn; delayed completion becomes Cancelled; sequential in-flight test permits only the first write |
| Rollback selector | Pure parser distinguishes agent-loop default activation and explicit legacy fallback |
| Scoped memory | Explicit preferences are persisted; `memory_search` is model-requested, kernel-gated, tenant/subject bound, result-capped, and isolated across two users |

## Test results

- `aelio-agent-loop`: 30/30 passed.
- `aelio-agent-api --lib`: 37/37 passed.
- `agent_loop_acceptance`: passed on a loopback deterministic gateway.
- `durable_idempotency_replays_result_after_restart`: passed.
- `unknown_non_idempotent_outcome_requires_manual_review`: passed.
- Agent-loop strict clippy: passed.
- Agent API library strict clippy: passed.
- TypeScript workspace: 16/16 tasks passed.
- Gateway contract, mock sequencing, web/WhatsApp parity, and production-authority graph: passed.
- Full local umbrella: passed through confirmation, authentication, identity, and two-user isolated memory recall.

## Remaining external evidence

E30 still requires a real production rollout record: staged assignment, safety-counter observation, rollback of new-conversation assignment, and confirmation that no uncertain write was replayed across authorities. That is an operational exercise, not missing repository code.

The repository-wide legacy and Sol baselines remain separately recorded in [`../../IMPLEMENTATION_AUDIT.md`](../../IMPLEMENTATION_AUDIT.md).
