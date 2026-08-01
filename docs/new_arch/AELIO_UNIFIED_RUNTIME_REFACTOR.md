# Aelio unified runtime refactor

Status: implemented production boundary.

## Invariant

There is one authoritative decision engine: the Rust implementation of
`AELIO_DSL_MOTHER.md`. TypeScript may translate external I/O, but it may not independently select a
pathway, interpret a flow, authorize an effect, mutate durable agent state, or decide whether an
artifact is promoted.

## Target process boundary

```text
browser / WhatsApp / Convox SDK
              |
              v
TypeScript edge
  HTTP + WebSocket + provider adapters only
              |
      versioned host protocol
              |
              v
Rust aelio-runtime
  turn actor -> Sense -> flow gate -> Planner/Executor -> Learn -> Express
              |
              v
embedded Aelio database
  WAL + rows + vector + BM25 + graph + continuations + ledger
```

The TypeScript edge owns:

- public HTTP/WebSocket termination and channel-specific signature verification;
- Convox SDK connections and execution of reverse tool frames;
- LLM/embedding vendor HTTP adapters and tenant-key access;
- translation between external payloads and closed, versioned Aelio host frames.

The Rust runtime owns:

- hydration order and frozen `sense`;
- flow/pathway selection and every deterministic decision;
- prompt/template selection and validation of model results;
- tool authorization, idempotency, deadlines, effect accounting and replay;
- conversion/procedure/pathway learning and the generic promotion gate;
- all durable state, ledger, continuation and artifact lifecycle transitions;
- response semantics. TypeScript delivers the response but does not rewrite it.

SDK tool results cross the authority boundary through a closed output contract. Each exposed tool
declares stable output names, source paths, types, sensitivity and meaning. Rust uses that
declaration as the only evidence allowlist for cold interpretation, learned signature extraction,
memory and synthesis; undeclared fields never reach an LLM or durable evidence.

## Rust workspace

`aelio-os/` becomes the complete Rust product workspace:

```text
aelio-os/crates/
  aelio-sol
  aelio-kernel
  aelio-convert
  aelio-learn
  aelio-query
  aelio-prompt
  aelio-store
  aelio-wire
  aelio-runtime
  aelio-server
  aelio-agent
  aelio-agent-api
  aelio-db-format
  aelio-db-wal
  aelio-db-index
  aelio-db-text
  aelio-db-graph
  aelio-db-cost
  aelio-db-engine
  aelio-db-query
  aelio-db-catalog
  aelio-db-storage
```

The product and public API use “Aelio database” or “Aelio DB.” Historical database and crate names
are not production aliases.

The service exposes two Rust-owned rails with one authority boundary:

- `aelio-kernel` + `aelio-runtime` execute immutable, explicitly registered Mother-DSL flows;
- `aelio-agent` executes adaptive conversational turns, recall, learning, policy and proactive work.

They run in the same `aelio-server` process, use the same Aelio database engine, and keep separate
durability namespaces (`runtime`, `agent`, `database`) so a catalog or WAL failure in one domain
cannot corrupt another. TypeScript never chooses between learned paths, advances a flow, authorizes
an effect or promotes an artifact.

Multimodal database recall uses the Rust-owned [Prism query language](PRISM_QUERY_LANGUAGE.md).
`POST /v1/prism` accepts its closed envelope and executes text, vector, graph, and scalar operators
inside the embedded Aelio DB; TypeScript does not compile or reinterpret Prism queries.

## Host protocol

The local TypeScript/Rust connection is closed, authenticated and versioned. Required frame
families:

- `turn.submit`, `turn.result`, `turn.error`;
- `model.call`, `model.result`;
- `embedding.call`, `embedding.result`;
- `tool.call`, `tool.result`;
- `channel.deliver`, `channel.result`;
- artifact registration and lifecycle commands;
- health/readiness and graceful drain.

Every request has `corr`, `tenant`, deadline and protocol version. Results are deduplicated by
`corr`. Secrets remain on the TypeScript side; Rust receives only the result and safe provider
metadata needed for its ledger.

## Migration gates

1. Database crates compile and test inside `aelio-os` under `aelio-db-*` names.
2. `aelio-store` has a real embedded database implementation; `MemoryStore` remains test-only.
3. Rust runtime can execute, park, restart, resume and replay the login golden flow through the
   embedded database.
4. TypeScript and Rust run shadow parity on the existing scenario suite. Rust output governs only
   after mismatches are resolved.
5. Widget, WhatsApp, SDK tool, live LLM and embedding paths all pass through Rust.
6. TypeScript decision modules are absent from the production server dependency graph. Historical
   libraries may remain as non-authoritative compatibility packages until their public exports are
   retired.
7. Legacy names are removed from configuration, environment variables, packages, images,
   documentation and telemetry, with explicit one-release config migration errors rather than
   silent fallback.

## Non-negotiable release tests

- kill after effect intent, dispatch and result independently;
- kill while parked and resume under the pinned flow/kernel versions;
- duplicate message, duplicate tool result and late tool result;
- concurrent turns for one instance and across tenants;
- tenant-isolation attempts in row, vector, text and graph retrieval;
- provider timeout/cancellation and SDK disconnect/reconnect;
- ledger replay bit identity;
- old-name configuration produces a clear migration diagnostic;
- no TypeScript code path can execute a turn without the Rust runtime.
