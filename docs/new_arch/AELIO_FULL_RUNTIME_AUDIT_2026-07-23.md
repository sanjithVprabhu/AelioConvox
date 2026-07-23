# Aelio full runtime audit — 2026-07-23

## Verdict

The implemented Rust core now matches the central architecture: deterministic selection, bounded
recall, typed execution, policy-wrapped effects, durable learning, immutable procedure versions,
reviewed flow promotion, restart recovery, and human-readable decision traces are connected to the
live turn path. The audit did not find another worked-example-only router or a hardcoded Tier-2
goal.

This is a core-runtime release verdict, not a claim that deployment-product decisions are finished.
The explicitly open deletion, calibration, pricing, channel, and horizontal-topology decisions are
listed below and must not be represented as implemented.

## Release defects found and corrected

1. Turn idempotency keys were not bound to the original user and payload. A duplicate key could
   replay another request's result. Durable rows now store a non-plaintext request hash and reject
   cross-user or changed-payload reuse.
2. Tool idempotency omitted the tool version. A redeployed tool could replay a result created by an
   old version. Version is now part of the canonical key.
3. The production binary bootstrapped with the demo CRM/OTP catalog and scripted test LLM. It now
   starts with an empty tenant and a fail-closed unavailable provider; turns return `Unavailable`
   until a complete catalog is registered.
4. Synchronous LLM/tool work ran directly on the asynchronous HTTP executor. Turn execution now
   runs on Tokio's blocking pool while retaining the serialized tenant runtime.
5. Proactive jobs were published before the daily-budget CAS. Concurrent candidates could both
   enqueue while one caller received `Conflict`. Budget/cadence is now reserved first, so a crash
   may under-send but can never over-send.
6. Durable proposals depended on every tenant tool and only the proposal prompt. Dependencies now
   contain the exact path tool versions and a fingerprint of prompts that can affect execution.
   Dependency invalidation also runs during restart, catching code/prompt drift without requiring a
   catalog republish.
7. Promoted proposals stopped learning. Post-promotion evidence now accumulates in bounded batches,
   creates immutable v2/v3 versions with `supersedes`, retains old versions for rollback, and loads
   the strongest valid version deterministically.
8. Shadow comparisons were recorded but could not improve a candidate. Shadow evidence now feeds
   the same re-promotion cycle and refreshes the active winner when a new version clears the gate.
9. Learning evidence recorded latency and token cost as zero. Actual wall time and reported model
   tokens now feed behavioral evidence. Monetary microunits remain zero until a tenant/provider
   pricing declaration exists.
10. Offline hash embeddings could accidentally authorize a novel business synonym through cosine
    or weak character overlap. Only embedders explicitly declaring semantic-equivalence support can
    bridge novel terms; hash mode now fails closed and lists declared attributes.
11. Catalog validation allowed flow steps to reference missing tool capabilities and lacked coarse
    collection/execution bounds. Complete snapshots now reject those shapes before publication.
12. Approval records could be pre-seeded for nonexistent future proposal IDs. Approval now requires
    an existing, non-rejected proposal.
13. The learning control plane was not exposed end to end. Authenticated APIs now list proposals
    and immutable procedures, approve/promote proposals, report active versus suspended versions,
    and provide a procedure kill switch.
14. Runtime/SDK credentials implicitly had administrative learning authority. Production supports
    separate `AELIO_API_KEYS` and `AELIO_ADMIN_API_KEYS`; non-admin credentials cannot reach
    `/v1/admin/*`, and open development mode never opens the admin surface.
15. Health always reported `ok`, and a production runtime could accept turns after catalog
    registration while its SDK tool host was disconnected. Health now reports catalog/auth/SDK
    readiness, and tool-bearing production tenants fail closed until the registered SDK is live.
16. The production binary could silently start with no authentication, and HTTP JSON bodies had no
    explicit application-level bound. Startup now fails closed unless credentials are configured or
    `AELIO_ALLOW_INSECURE_OPEN=1` is deliberately set; HTTP bodies are capped at 4 MiB and SDK
    WebSocket messages remain capped at 1 MiB.

## Architecture coverage

| Area | Audit result |
|---|---|
| Turn ordering and flow-first routing | Implemented and golden-tested |
| Tier 0 exact / Tier 1 vector / Tier 2 typed compose / Tier 3 closed model proposal | Implemented in the live spine |
| Deep-turn memory and document recall | Bounded, user/tenant scoped, grounded, live-wired |
| Tool bind/invoke/signature/extract/sanitize | Implemented; secrets and version drift tested |
| Policy at tool, transition, flow, memory, and document boundaries | Fail-closed for risky actions |
| Saga, CAS, leases, unknown non-idempotent outcomes | Implemented and restart/contention-tested |
| Cold learning and immutable version evolution | Implemented through v2 selection and rollback retention |
| Operator-budgeted shadow exploration | Implemented, default-off, read-only/model-free, restart durable |
| Candidate-flow promotion | Separate durable review state machine with protected-rail checks |
| Decision logs | Redacted turn-by-turn narrative verified with the scripted conversation |
| Production bootstrap and HTTP auth | Empty/fail-closed bootstrap and scoped admin credentials |

## Verification evidence

- `cargo test -p aelio -p aelio-server`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo run -p aelio --example decision_log_conversation`
- Restart, CAS races, corruption/fuzz, idempotency, prompt/tool invalidation, recall isolation,
  proactive caps, v1-to-v2 learning, protected-flow review, and admin-scope regression tests.

Network-backed live-LLM tests were not run in this audit because `OPENAI_API_KEY`,
`AELIO_LLM_ENDPOINT`, and `AELIO_LLM_GATEWAY_URL` were absent. They remain deliberately ignored,
not silently counted as passing.

## Explicitly unresolved product decisions

These are not hidden code failures; the architecture documents themselves leave them open.

1. User-wide GDPR/DPDP deletion, including whether aggregate procedures learned partly from a
   deleted user's behavior must be rebuilt or only lose attributable raw evidence.
2. Per-tenant calibration workflow and acceptance thresholds for every semantic/LLM escalation
   pair. Unit/golden tests exist; a statistical calibration product does not.
3. Provider and tool pricing declarations. Tokens, calls, and latency are auditable, but monetary
   cost cannot be truthful until pricing/version/effective-time semantics are declared.
4. Aggregate operational metrics required by H7: tier hit rates, escalation rate, disagreement
   rate, and cost per turn. Raw durable facts exist; the metrics/reporting surface is not built.
5. Entity identity resolution rules for entity memory. The taxonomy and storage projection exist,
   but merge/split authority remains undecided.
6. Locale/currency formatting as a declared ability.
7. Channel delivery contracts, SDK heartbeat policy, multi-node ownership, and horizontal scaling.
   The current executable topology is one tenant runtime serialized per server process.
8. Cross-tenant learning remains intentionally disabled.

These decisions require product/security choices before implementation. Guessing them in runtime
code would make the system less trustworthy, not more complete.
