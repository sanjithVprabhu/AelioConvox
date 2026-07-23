# Aelio production safety profile

This profile makes the architecture's safety defaults executable rather than conventional.

## Policy defaults

- Pure and read-only actions inside the current state permission envelope default to allow.
- Effectful tool calls, state transitions, flow activation/resumption, and document mutations
  default to deny unless an authored policy explicitly allows them.
- Any matching deny wins over every matching allow. Exact capabilities do not expand to child
  capabilities; wildcard expansion must be declared with `.*`.
- Tool calls, transitions, flows, and document mutations set their action-risk class at the
  structural boundary, so a caller cannot accidentally obtain the read-only default.

## Deadlines and unknown outcomes

- Pure/composed operations use intersecting cooperative deadlines; nested budgets can only narrow
  a parent deadline.
- A timeout after dispatch is never interpreted as proof that an external effect did not happen.
- An idempotent timed-out tool is stored as `unknown_outcome` and may only be retried with the same
  idempotency key.
- A non-idempotent timed-out tool is stored as `manual_review` and is never retried automatically.

## Grounded terminal output

- Terminal synthesis receives typed evidence claims with stable claim IDs and provenance.
- The provider's closed output contains both `text` and `claim_refs`.
- Every emitted reference is checked against the supplied claim set, and factual synthesis with no
  references fails closed.
- If model output fails validation, the runtime returns the verified evidence deterministically;
  it does not expose the invalid model completion.
- The earlier token-overlap groundedness score remains diagnostic only and is not an authorization
  or correctness gate.

## Multi-clause execution

- At most four independently actionable clauses are coordinated in one turn.
- Contradictory action pairs suspend for clarification.
- Read-only clauses execute first and remain independently traced.
- Mutating clauses are not executed; they are returned as an ordered confirmation plan.
- Clause order is preserved. Auth, payment, destructive, and other effectful steps are never
  silently reordered by the learner or executed from an unconfirmed multi-clause request.

## Compatibility and operating rule

Tenant catalogs that previously relied on implicit allow must add explicit allow policies for
effects, transitions, flows, and document writes. This is an intentional fail-safe migration: an
old catalog may stop a mutation, but it cannot silently broaden authority.
