# Aelio learning and recall: implemented runtime contract

This note records what the Rust runtime actually does. It complements the architecture explanation
and decision ledger; it is deliberately an implementation truth document rather than an aspiration.

## The live loop

```text
Sense → resume authored/synthetic flow → explicit control commands → clause gate → depth
      → situation key σ → bounded recall → Tier 0/1/2/3 → typed execution
      → grounded expression → durable outcome signal → promote/suspend → persist
```

The model is not the router. It may propose a path at cold Tier 3 and may express evidence at the
terminal boundary. Proposed paths use only declared ability identifiers, are parsed through a
closed output schema, type-checked, policy-wrapped, and supervised by the same generic executor as
promoted paths.

## Recall and organized memory

`StoreTurnRecall` assembles a tenant- and user-scoped evidence slice from factual memories and
virtual-document chunks. Retrieval is lexical first and uses semantic search only when lexical
results do not fill the bounded candidate set. Candidates are fused, stale document revisions and
invalid-time memories are removed, and the final slice is hard-limited by both hit count and
character count. Retrieval failure is visible as `Recall.Error` and safely falls back to the cold
path; it never fabricates a remembered fact.

Every result enters synthesis as an `EvidenceClaim` with an ID and provenance. Safe tool response
fields also become field-level claims carrying tool ID, response signature, and invocation identity.
Secret/redacted fields cannot become claims. Grounded synthesis returns a closed `claim_refs` list,
which is verified against the supplied claims.

Memory creation is explicit:

- `remember that …` and `remember: …` bypass the LLM and enter a policy-controlled durable write.
- Facts are factual-memory records with tenant/user ownership, valid time, provenance, confidence,
  and embeddings from the same configured embedding space used by recall.
- OTP-like six-digit values, bearer credentials, and oversized facts are rejected.
- `forget that …` and `forget: …` retire the exact memory with compare-and-set. Forgotten rows are
  retained for audit but are excluded from all recall channels.
- Ordinary conversation is never silently converted into durable personal memory.

Production deployments use `DurableRuntime::new_with_embedder` with the same semantic embedder used
to index tenant documents. The constructor rejects both dimension mismatch and persisted
embedding-space identity drift, including a same-dimension model or endpoint change. The
`aelio-server` binary fails startup unless `AELIO_LLM_EMBED_URL` (or the canonical TypeScript
gateway URL) configures a semantic embedder. Deterministic hash embeddings remain available only
through the library's offline/test constructor or the explicit
`AELIO_ALLOW_HASH_EMBEDDER=1` development override; they never authorize novel semantic
equivalence.

## Learning over time

Cold successful turns create durable proposals containing the real σ: state, intent, filled slots,
reachable capabilities, hash, and near-situation embedding. Outcomes come from behavior—not an LLM
self-grade—including completion, tool success/error, repair, flow terminal state, abandonment,
latency, and cost.

Promotion requires the configured observation, success-rate, and mean-score gates. Effectful paths
require explicit tenant approval. Promotion creates one immutable procedure version under CAS, with
tool and prompt dependencies, evidence, provenance, a compositional producer postcondition, and the
complete executable spec. On restart the registry is rebuilt from valid promoted versions.

Promoted paths continue receiving behavioral and shadow evidence. Each additional promotion-sized
batch can create an immutable v2/v3 with a `supersedes` link; prior versions remain available for
rollback, while the strongest valid version is selected deterministically. Dependencies contain
only exact path tool versions and prompt-backed execution abilities, and are rechecked on restart.

Warm lookup is:

1. Tier 0: exact σ hash.
2. Tier 1: near-situation vector match with an absolute score and winner margin gate.
3. Tier 2: bounded recursive backward composition using a typed `GoalSpec`, declared tool input and
   output evidence, available slots, capability envelope, effect permission, maximum procedures,
   and maximum steps. Multi-hop plans thread only declared sanitized evidence between tools.
4. Tier 3: one closed-vocabulary model proposal, followed by type-check and supervised execution.

Read-only procedures may promote automatically after behavioral evidence. Effectful procedures do
not. A promoted procedure is suspended immediately after a tool error or dependency invalidation;
immutable history is retained. Durable promotion tests prove that repeated cold success becomes a
zero-LLM Tier-0 turn and survives database restart.

Ranked attribute requests use this same ladder. Parsing produces only a bounded attribute,
direction, and limit plan; the attribute declaration supplies the query capability. The resulting
typed path is observed, promoted, and later retrieved at Tier 0 like any other read-only procedure.
Tool payload fields become grounded answer claims only when their `OutputSpec` declares them;
undeclared response fields are discarded, not passed to synthesis.

## Vocabulary learning and exact continuation

Thin semantic term margins park an exact `__aelio.term_confirmation` plan. A later `yes` executes
the persisted declared attribute, direction, limit, and capability; it does not reinterpret the
original phrase. Three distinct hashed-user confirmations promote the term into the tenant's
declared learned synonyms. The updated active catalog is durable and reloads after restart.

Multi-action confirmations similarly persist the ordered clauses. If an action suspends inside a
flow, the remaining clauses and next index are carried in protected flow metadata. Completion of
the nested flow resumes the exact remaining plan without reordering or reconfirming completed work.

## Failure and restart behavior

- Turns are claimed idempotently. Completed results replay without re-running tools.
- Failed turns persist a redacted typed error instead of remaining permanently `processing`.
- Tool calls have durable leases and sanitized outcomes.
- A non-idempotent timeout becomes manual review because the external outcome is unknown.
- Active tenant catalogs, user state, flows, promoted procedures, memories, and evidence reload
  after restart.
- Decision-log steps are redacted before durable storage and human-readable rendering.

## Controlled shadow exploration

Runner-up comparison is implemented, but remains disabled by default. An operator can configure it
through `POST /v1/admin/exploration` or `DurableRuntime::configure_exploration`. The policy requires
a deterministic sample rate, a durable maximum-trials-per-window limit, and a maximum candidate
path length. Concurrent workers claim the shared window with compare-and-set, so a race cannot
exceed the cap and a restart cannot reset it.

Only a valid immutable runner-up for the exact situation can run. The shadow executor rejects
effectful paths, LLM-backed abilities, suspension, and state mutation. It cannot replace the served
reply. Its result is stored as an `ExplorationComparison`, contributes a behavioral signal, and is
rendered as `Explore.Shadow` in the decision log. Candidate selection and traffic sampling are
deterministic; no unseeded randomness enters the live path.

## Review-only candidate flows

Learned sequences cannot silently rewrite the tenant's control plane. Candidate flows are stored in
a separate durable namespace and move through:

```text
pending_review → approved → activating → active
              ↘ rejected       ↘ rejected (terminal gate failure)
```

Proposal requires 1–32 valid immutable source procedures with promotion-grade evidence. Every
admissible flow capability must occur in a source path, resolve to exactly one current tool, and
that exact tool/version must be present in the source dependency proof. Candidate structure,
closed predicates, terminal states, trigger bounds, step bounds, and the complete prospective
tenant catalog are validated before storage. Stored candidates are frozen as `learnable=false`.

Authored `learnable=false` rails and overlapping trigger surfaces cannot be replaced, including at
activation time. Approval/rejection is an authenticated, auditable compare-and-set operation;
concurrent reviews have one winner. Activation rechecks source invalidation and protected rails,
publishes the catalog durably, and is restart-idempotent if a worker crashes while activating.
Terminal gate failures durably reject the candidate instead of leaving it stuck in `activating`.
Admin routes fail closed when the server is otherwise running in open development mode, and audit
ownership records a non-secret fingerprint of the authenticating API key plus the reviewer label.
Production separates ordinary runtime/SDK credentials (`AELIO_API_KEYS`) from learning-control
credentials (`AELIO_ADMIN_API_KEYS`).
The server fails closed when both are absent. Unauthenticated local development must be explicitly
enabled with `AELIO_ALLOW_INSECURE_OPEN=1`; even then, administrative routes remain unavailable.

The proposal/procedure control surface is:

- `GET /v1/admin/proposals`
- `POST /v1/admin/proposals/{id}/approve`
- `POST /v1/admin/proposals/{id}/promote`
- `GET /v1/admin/procedures`
- `POST /v1/admin/procedures/{id}/suspend`

The authenticated server surfaces are:

- `GET/POST /v1/admin/flow-candidates`
- `POST /v1/admin/flow-candidates/{key}/review`
- `POST /v1/admin/flow-candidates/{key}/activate`

## Verification

From `Sunjet/Astrolobe`:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The deterministic decision-log conversation remains available with:

```bash
cargo run -p aelio --example decision_log_conversation
```

The test corpus includes cold-to-warm promotion, restart recovery, promotion races, dependency and
behavioral invalidation, bounded recall, user isolation, explicit remember/recall/forget, secret
rejection, exact term confirmation and synonym promotion, multi-action suspension, OTP repair,
unknown non-idempotent outcomes, storage recovery, corruption/fuzz handling, strict linting,
shared exploration-cap races, shadow safety/restart behavior, protected-flow rejection,
single-winner review races, candidate activation, real flow entry, and post-restart catalog reload.
