# P4 agent-loop default integration evidence

**Date:** 2026-08-09  
**Checkpoint:** Code/default-launch integration complete; external production observation pending  
**Authority under test:** `AELIO_HARNESS_MODE=agent_loop`

## Automated results

- `aelio-agent-loop`: 30/30 tests passed after the hardening pass.
- `aelio-agent-api --lib`: 37/37 tests passed after the hardening pass.
- `agent_loop_acceptance`: end-to-end missing-input, confirmation, lifecycle, protected-read, and concurrent-cancellation path passed.
- New crate strict clippy: passed with `-D warnings`.
- Full TypeScript workspace typecheck/build dependency graph: 16/16 tasks passed across 12 packages.
- Production authority graph: passed; Rust remains the only reachable turn decision executor.
- Gateway v2 strict contract: passed.
- Deterministic native-tool mock sequence: passed.
- Web/WhatsApp agent-loop channel parity: passed.
- Full local `agent_loop` umbrella: passed, including two-user scoped memory recall.
- Complete non-API Rust workspace: passed; one intentionally ignored test.
- `git diff --check`: passed.

## Live results

| Scenario | Observed result |
|---|---|
| Read loop | HTTP 200; model calls 2; SDK tool calls 1; accepted finish |
| Duplicate ingress | Cached result returned; altered payload for same turn ID returned 409 |
| Confirmation park | Suspended; model calls 1; SDK tool calls 0 |
| Ambiguous consent | Suspended; model calls 0; SDK tool calls 0 |
| Exact approval | One write dispatched, then model finish |
| Repeated later approval | No parked effect replayed |
| Runtime restart | Conversation resumed; fencing token advanced |
| Readiness | `agentModelGateway` reported ready |
| Capability negotiation | Gateway v2/native-tools handshake accepted |

## Preserved red baselines

- Three legacy API compatibility assertions remain red; see [`../../IMPLEMENTATION_AUDIT.md`](../../IMPLEMENTATION_AUDIT.md).
- The checkout now selects `agent_loop`; the post-hardening umbrella suite is green and recorded in the implementation audit.
- Whole-workspace `-D warnings` remains blocked by pre-existing warnings outside the new crate.

## Cutover and rollback

- Standard launcher default when unset: `agent_loop`.
- Explicit activation: `AELIO_HARNESS_MODE=agent_loop pnpm start`.
- New-conversation fallback: `AELIO_HARNESS_MODE=legacy pnpm start`.
- Active conversations and uncertain effects must not be translated or replayed across authorities.

## Audit pointer

The implementation matrix, invariant review, E01–E30 disposition, and production-certification gaps are in [`../../IMPLEMENTATION_AUDIT.md`](../../IMPLEMENTATION_AUDIT.md).
The follow-up gap-closure evidence is in [`../P4_HARDENING/summary.md`](../P4_HARDENING/summary.md).
