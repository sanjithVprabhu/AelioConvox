# aelio — typed ability runtime

Rust implementation of the Aelio / Convox architecture (`docs/new_arch/`).

## Core bet

**Retrieval and selection are deterministic.** The LLM proposes ordered paths over a
typed ability set (and synthesizes terminal speech). The type system is the verifier.

## Layout

| Module | Rung | Role |
|--------|------|------|
| `types`, `contract` | — | `Value`, `ReasonCode`, uniform ability contract |
| `ops::pure` | L0 S0-B | Pure ops (numeric, string, path, validate, repair) |
| `ops::combinators` | L0 S0-A | `Seq`, `Loop`, `Guard`, `Once`, `Budget`, `Park`, … |
| `ops::effects` | L0 S0-C | `Now`, `Uuid`, `LedgerAppend` (replay-friendly) |
| `abilities::*` | L1 | Sense, Understand, Judge, Bind, Invoke, Sig, Express, State, Registry, Learn |
| `policy` | L1/L5 | Closed predicate grammar + `Policy.Evaluate` invariant |
| `tenant` | L5 | ToolSpec, FlowSpec, StateSpec, PolicySpec, Personality, attributes |
| `blocks::*` | L2 | Turn spine, tool-call block, flow control, term resolution |
| `runtime::World` | — | In-memory demo tenant + turn driver |

## Hot loop (every turn)

```
sense → flow gate → clause split (pre-gate) → triage
      → situation key (bucketed) → tier 0/1/2/3
      → execute → synthesize → write back
```

## Determinism budget (enforced in golden traces)

| Turn | LLM calls |
|------|-----------|
| `Hi` cold | 1 (ProposePath) |
| `Hi` warm | **0** |
| `Hi` mid-OTP | **0** (resume) |
| shallow template | 0 after promotion |

## Tests

```sh
cargo test -p aelio-agent
```

Golden traces cover: cold/warm greeting, mid-flow resume, login capability lookup,
OTP resume, avaricious term resolution, binder residuals, policy deny, signature
mismatch, tool invalidation cascade, tier-2 composition.

## Durability

The runtime persists turns, flow instances, lifecycle state, procedures, proposals,
provider receipts, memory, proactive jobs and audit records through `AelioStore`,
backed by the embedded Aelio database crates in this workspace. The durable runtime
uses caller-stable commands, compare-and-swap leases and replay-safe effect receipts;
the in-memory `World` remains available only for focused tests and examples.
