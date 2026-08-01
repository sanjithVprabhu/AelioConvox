# Aelio Unified Implementation Master Plan

**Status:** implementation-convergence specification, v0.1 (2026-08-01)

**Objective:** turn the locked Aelio DSL kernel, the Harness technical specification, Prism,
the adaptive agent, Aelio DB, the Rust server, and the TypeScript edge into one end-to-end system
with one execution authority and no duplicate decision engine.

**Authority:** `AELIO_DSL_MOTHER.md` remains the highest authority. This document resolves how the
technical mother is applied to the current repository. Where this document proposes an amendment
to a locked mother section, that item is marked **RATIFICATION REQUIRED** and must be copied into
the mother Decision Log before the affected behavior is called normative.

**Definition of success:** a user request can enter through a real channel, be routed to the Rust
authority, execute an existing pinned artifact or create an off-path build demand, use prompts and
tools through constrained boundaries, persist every durable decision in Aelio DB, park/restart/
resume, replay without divergence, learn only through evidence gates, and return through the same
channel. No TypeScript code and no parallel Rust subsystem may independently authorize an effect,
advance a flow, or promote learned logic.

---

## 1. The system being built

Aelio is not an LLM wrapper and not a general autonomous-code generator. It is a bounded agent
operating system with three distinct responsibilities:

1. **Authoring:** models may search, select, compose, decompose, and propose artifacts.
2. **Guaranteeing:** deterministic Rust code validates shapes, bounds, effects, policies, pins,
   examples, evidence, and lifecycle transitions.
3. **Executing:** one Rust reactor runs only pinned, admitted artifacts and records every source of
   nondeterminism needed for replay.

The fundamental flow is:

```text
external message / scheduled wake / SDK event
                    |
                    v
TypeScript edge: authenticate channel, hold provider/tool credentials, translate frames
                    |
                    v
Rust aelio-server: authenticate tenant and protocol
                    |
                    v
Sense + active-flow gate + pinned artifact resolution
          | existing                         | missing
          v                                  v
deterministic reactor                  insufficiency record
          |                                  |
          |                            off-path build queue
          |                                  |
          |                           root harness builder
          |                         search/compose/decompose
          |                                  |
          |                         validate + sandbox + gate
          |                                  |
          +<----------- pinned artifact <----+
          |
          v
prompt / Prism recall / reverse tool call / sub-artifact
          |
          v
ledger + state + continuation + evidence in Aelio DB
          |
          v
channel response
```

Runtime never executes an artifact created during the same user turn. A missing capability produces
an honest decline or bounded fallback and an off-path demand record. This separation is mandatory:
it prevents an unreviewed model proposal from becoming live behavior in one breath.

---

## 2. Canonical convergence decisions

These decisions eliminate contradictions between the two mother documents and the current code.

### C-001 — Authority order

Order of authority:

1. locked `AELIO_DSL_MOTHER.md`, including later Decision Log amendments;
2. this convergence document after ratification of marked amendments;
3. `TECHNICAL_MOTHER_SPECIFICATION.md`;
4. annexes and handoff documents;
5. implementation comments and historical documents.

No lower source may silently override a higher source. A contradiction becomes a `FLAGS.md`
finding and a test marked ignored only when external infrastructure is genuinely required.

### C-002 — Canonical Sol serialization

The DSL mother wins:

- preserve raw Unicode code points; perform no NFC/NFD normalization;
- sort map keys by their encoded key ordering as implemented by `aelio-sol`;
- preserve integer versus float identity;
- render negative float zero as `0.0`, not integer `0`;
- reject NaN and infinity;
- use BLAKE3 over canonical bytes.

The superseded NFC and `-0 -> 0` language was removed from Technical Mother Appendix G.1 during
Phase 0 convergence.

### C-003 — One path grammar

The mother §6.1 grammar is canonical: identifier segments, quoted-bracket keys, integer indexes,
literal paths only, maximum depth 32. Technical `$input`, `$output`, and `$<nid>` paths are
**authoring sugar**. The builder compiler resolves them to ordinary literal Sol paths before the
Mother Planner sees a program. Runtime instructions never carry a second path language.

### C-004 — Prism is the canonical flexible-read language

**RATIFICATION REQUIRED for mother §10.4 / existing F-018.**

The implemented closed Prism envelope becomes the canonical multimodal flexible-read AST:

```json
{
  "from": "literal_collection_id",
  "match": ["key|text|vector|graph|fusion clauses"],
  "where": ["closed scalar predicates"],
  "select": ["mandatory literal columns"],
  "limit": "mandatory 1..10000",
  "into": "optional at database boundary, mandatory when used by Call"
}
```

Rules:

- `from`, matched columns, predicate columns, and selected columns are literal and schema-checked;
- at most one text, vector, graph, and fusion clause in v1;
- multiple ranked modalities require exactly one explicit RRF fusion clause;
- graph requires bounded depth, node count, and seeds;
- no joins, expressions, subqueries, arbitrary query text, or model-generated field names;
- the legacy single-operation `QueryAst` remains internal sugar that lowers into Prism;
- `order`, cursor pagination, `all`, and general range scans remain absent until separately bounded
  and ratified. They must not be documented as shipped behavior before implementation.

### C-005 — Replay injects external observations

Model results, tool results, channel inputs, Sense snapshots, nondeterministic values, and database
read/Prism results are recorded and **injected** during replay. Their argument hashes are verified.
Pure kernel calculations and conversions are recomputed and hash-verified. A database read is not
re-executed during historical replay because snapshots, indexes, and rankings may have changed.

### C-006 — Reaction and ledger relationship

One reaction is one logical step checkpoint, but a reaction may produce multiple ordered ledger
entries. For example an external write reaction produces `call_intent`, `call_dispatch`, and
`call_result`. Every entry carries `reaction_id`; replay consumes the low-level sequence. This
preserves Technical Mother D-5 without discarding the mother App G crash-window protocol.

### C-007 — One artifact lifecycle

The mother Appendix K lifecycle is canonical:

```text
proposed -> rejected
proposed -> shadow -> canary -> promoted
                         |          |
                         +-> shadow <-+  demotion
any active state -> retired
```

- shadow to canary: at least 20 distinct inputs and at least 95% validation;
- reviewed artifacts require approval **before first consumption**, at shadow-to-canary;
- canary to promoted: at least 20 distinct canary inputs, at least 98% downstream success,
  zero attributed Guard violations, and no unresolved semantic-review hold;
- any Guard violation or failure rate above 2% demotes immediately;
- fabrication, PII, and downstream write/external reach force reviewed classification;
- secret-bearing proposals are locked and never automatically proposed.

The superseded Technical D-28 `K=20/M=5` rule was removed during Phase 0 convergence. Any future
class-specific evidence minimum may only strengthen, never weaken, the generic lifecycle above.

### C-008 — One execution authority

`aelio-kernel` is the only instruction interpreter. `aelio-runtime` is the only artifact reactor.
`aelio-agent` may understand messages, retrieve evidence, score declared alternatives, identify
missing capability, and generate build demand. It must progressively stop executing independent
effectful procedures. An adaptive choice becomes either:

1. a pinned artifact invocation executed by `aelio-runtime`; or
2. an insufficiency record handled off-path.

During migration the old agent rail may run in shadow for parity, but only one rail may be
authoritative for a tenant/turn and that authority must be ledgered.

### C-009 — One database engine, isolated logical namespaces

All durable data uses the embedded Aelio DB engine. Separate physical directories or logical
namespaces for engine recovery isolation are allowed, but they are not separate products. Catalog,
backup, observability, tenant isolation, and migration are controlled by the same Rust server.

### C-010 — Prompt refusal shape

Every prompt forged by Mint must have a declared refusal/`undeterminable` variant appropriate to
its output imprint. Hand-authored operational prompts may use a more specific declared refusal
variant, but may never lack a valid non-fabricating outcome. Refusal is data, not an exception;
model parse failure remains an error with one bounded retry.

### C-011 — Off-path building only

Builds and repairs run from control-plane requests or the demand queue, never inline in a live
turn. Runtime may select only an already pinned canary/promoted artifact allowed by tenant policy.
New artifacts default to canary after their initial sandbox gate.

### C-012 — TypeScript boundary

TypeScript owns public channel adapters, provider HTTP calls, SDK connections, and customer-held
tool credentials. It must not contain fallback pathway selection, flow advancement, effect policy,
artifact promotion, or durable agent-memory decisions. Every request crossing into Rust is closed,
versioned, authenticated, tenant-scoped, correlated, and deadlined.

---

## 3. Final component ownership

### 3.1 `aelio-sol`

Owns only representation guarantees:

- `SolValue`, fundamental types, program-bearing detection;
- canonical serialization and BLAKE3 hashing;
- path parsing and literal-path limits;
- structural imprints and size/depth/list/map limits.

It has no internal Aelio dependencies and no database, model, or policy knowledge.

### 3.2 `aelio-kernel`

Owns deterministic guarantees:

- closed App E instruction parser;
- Planner static checks and read/write derivation;
- sequential Executor and later legal wave execution;
- Control and Compute operations;
- policy/effect invariants at Call boundaries;
- ledger entry construction and replay verification;
- Once, Park/resume, continuation codec, and termination turns;
- target declaration and invocation contract.

It must not search for capabilities or ask a model what to do.

### 3.3 `aelio-query` and `aelio-db-*`

Own Prism parsing, collection schemas, plan lowering, query execution, indexes, WAL, storage, and
database HTTP compatibility. Prism validation occurs twice: at artifact push/build and against the
physical catalog immediately before execution.

### 3.4 `aelio-store`

Owns typed durable repositories over Aelio DB, not business decisions. Expand its current generic
CAS/scan interface into named repositories for artifacts, ledgers, continuations, evidence,
builds, leases, and insufficiencies. `MemoryStore` remains test-only.

### 3.5 `aelio-prompt` / Mint

Owns immutable templates, layers, slots, model pins, exemplars, composition, prompt hashes, output
imprints, and the human-authored `root` axiom. Mint authors prompt artifacts; it cannot execute or
promote them.

### 3.6 `aelio-convert` / Glu

Owns seam validation, the closed conversion-rule set, edge-scoped reuse, proposal containment,
generic lifecycle primitives, evidence accumulation, and mutation testing. A converter is never
authored inline by the reactor.

### 3.7 `aelio-learn`

Owns pathway hygiene, procedure mining, attribution, evidence aggregation, drift metrics, and kill
criteria. It proposes lifecycle events; it does not directly write promoted status.

### 3.8 `aelio-runtime`

Becomes the application-level authority and contains five modules:

```text
aelio-runtime/src/
  artifact/       canonical Artifact, BuildSpec, pins, trust, lifecycle records
  builder/        twelve-step root harness and deterministic k.* operations
  reactor/        current flow execution, actors, hydration, Park/resume
  sandbox/        isolated example fixtures and gate runs
  steward/        dependency cascades, demand queue, promotion proposals
```

The existing runtime APIs are migrated into these modules without replacing the tested kernel.
Forge becomes an adapter into `builder`, not a separate promotion path.

### 3.9 `aelio-agent`

Becomes the conversational intelligence client of the runtime:

- Sense enrichment and bounded understanding;
- retrieval of evidence and pinned candidate artifacts;
- clarification and deterministic fallback selection;
- production of a typed `CapabilityRequest` when no pinned artifact applies;
- outcome signals for the learning subsystem.

Effectful execution, flow persistence, promotion, and continuation ownership move to the reactor.

### 3.10 `aelio-wire`, `aelio-server`, and TypeScript

`aelio-wire` owns protocol frames and delivery state. `aelio-server` owns authentication, routing,
readiness, rate/size limits, and composition of all Rust services. TypeScript implements only the
other end of `aelio-wire` plus channel/provider/SDK adapters.

---

## 4. Canonical durable model

Every collection is tenant-scoped at the repository boundary. A missing tenant is an internal
error, never a global fallback. The physical schema may use hashed tenant table names or a leading
tenant key, but cross-tenant scans must be structurally impossible through public repositories.

### 4.1 Required collections

| Collection | Key | Purpose |
|---|---|---|
| `artifacts` | `(tenant,id,version)` | immutable artifact body, interface, pins, trust |
| `artifact_history` | `(tenant,id,version,seq)` | lifecycle transition audit |
| `registry` | `(tenant_or_vendor,class,id,version)` | admitted target/dataset/imprint/template declarations |
| `flows` | `(tenant,flow_id,flow_rev)` | compiled pinned program and plan metadata |
| `flow_instances` | `(tenant,instance_id)` | CAS-owned execution state |
| `continuations` | `(tenant,instance_id,continuation_hash)` | App I continuation plus pins |
| `ledger` | `(tenant,instance_id,seq)` | append-only, gapless, hash-chained entries |
| `states` | `(tenant,scope,key)` | versioned tenant/session/user state |
| `messages` | `(tenant,session,turn,index)` | channel inputs and outputs |
| `conversion_edges` | canonical edge id | rules, lifecycle and evidence |
| `artifact_evidence` | `(tenant,artifact,input_hash)` | distinct-input shadow/canary outcomes |
| `rejected_proposals` | `(tenant,class,proposal_hash)` | rejection and backoff |
| `insufficiency_log` | `(tenant,dedup_hash)` | ranked missing-capability signal |
| `build_jobs` | `(tenant,build_id)` | durable root-harness state machine |
| `name_leases` | `(tenant,name,interface_hash)` | concurrent build exclusion |
| `verification_cache` | `(tenant,candidate_hash,examples_hash,pin_context_hash)` | sandbox verdict cache |
| `promotion_proposals` | `(tenant,artifact,proposal_id)` | evidence and required approval |
| `metrics` | `(tenant,metric,window,labels_hash)` | learning economics and operations |

### 4.2 Artifact record

The canonical artifact is closed and versioned:

```json
{
  "id": "literal stable id",
  "version": 1,
  "class": "flow|harness|prompt|derived_op|glu|imprint|dataset|pathway|procedure",
  "sol_version": "1",
  "kernel_version": "pinned",
  "interface": {"inputs": [], "output": "declared.imprint@1"},
  "description": "authoring metadata only",
  "effects": ["pure|read|write|external"],
  "trust": "debris|held|shadow|canary|promoted|retired",
  "pins": {
    "targets": [], "artifacts": [], "prompts": [], "models": [],
    "embeddings": [], "converters": [], "datasets": []
  },
  "examples": [],
  "body": {},
  "hash": "blake3 canonical identity fields",
  "provenance": {}
}
```

Artifact identity excludes mutable evidence, counters, descriptions used only for search metrics,
and operational timestamps. Editing executable body, interface, examples, or pins creates a new
version. No reference to `latest` exists in an executable artifact.

### 4.3 Durability invariants

- ledger append precedes any externally visible reply;
- effect intent is durable before dispatch;
- dispatch is durable when the frame leaves the server;
- result is durable before the flow continues;
- a continuation and its `park` ledger entry commit together or recovery refuses it;
- lifecycle status and history event commit together;
- CAS owns one active writer per flow instance;
- every bounded scan has an explicit limit;
- every repository method requires tenant explicitly.

---

## 5. Root harness: exact build state machine

The root harness is a durable build job, not a single HTTP handler stack. Every transition writes
the job state, budget consumption, relevant hashes, and ledger reference before dispatching the
next model or sandbox action.

### 5.1 Input

`BuildSpec@1` contains:

- stable name and one free-text objective/description;
- declared input slots and pinned imprints;
- pinned output imprint;
- copied structural caps: `max_depth`, `max_children`;
- shared consumable caps: LLM calls, tokens, reactions, wall time;
- tenant registry scope and principal grants;
- allowed/denied effect sets;
- at least three examples, including one valid refusal/negative case;
- optional synthetic fixtures for recall-bearing examples.

### 5.2 State transitions

1. **Admit**
   - parse closed schema;
   - enforce limits and example coverage;
   - resolve every imprint pin;
   - verify policy is a subset of principal grants;
   - atomically reserve `(tenant,name,interface_hash)`;
   - compute normalized `spec_hash`.

2. **Resolve exact/in-flight**
   - identical promoted artifact by `spec_hash` returns `reused`;
   - identical active build subscribes to that job;
   - verification-cache hit may reuse the cached sandbox verdict;
   - similarity alone can never return executable reuse.

3. **Search**
   - run closed Prism text+vector RRF over promoted/canary artifacts allowed by effects and trust;
   - use pinned embedding space;
   - return at most 25 candidates at level 0;
   - one deterministic widen step is permitted.

4. **Select**
   - invoke pinned `prompt.select@1` with only the candidate table and declared spec fields;
   - parse against `selection_result@1`;
   - reject invented ids;
   - apply thresholds to measured similarity, not model confidence;
   - empty or low-margin selection becomes abstention.

5. **Compose**
   - invoke pinned `prompt.compose@1` over full selected interfaces;
   - parse a closed draft tree and seam list;
   - model cannot emit converter rules or unpinned target ids.

6. **Validate draft**
   - compile through the real Mother Planner;
   - classify errors exactly:
     - unknown target: one widen attempt, then decompose;
     - seam mismatch: one recomposition with diagnostic, then decompose;
     - effect, cycle, policy, path, or bound violation: terminal failure;
   - no model retry may negotiate a safety invariant.

7. **Decompose**
   - invoke pinned `prompt.decompose@1` without repeating the failed candidate list;
   - require strictly smaller child complexity and complete typed interfaces;
   - `atomic` in v1 means fail and log insufficiency, never invent a primitive.

8. **Admit children**
   - enforce breadth/depth;
   - reject ancestor oscillation;
   - prove interface closure across the seam graph;
   - reserve child names and pass the same atomic consumable budget handle.

9. **Recurse**
   - build children in deterministic topological order;
   - collect all gaps while budget remains;
   - no partial child set may be assembled as success.

10. **Assemble**
    - substitute pinned child artifacts;
    - resolve and pin Glu converters;
    - compute transitive pins and effect union;
    - cap trust to the minimum pin trust;
    - deduplicate identical subtrees by canonical hash;
    - emit a Mother-DSL program and compile it with the real Planner.

11. **Sandbox gate**
    - create a unique namespace;
    - load only declared synthetic fixtures;
    - execute every example through the production reactor with external calls stubbed by declared
      deterministic fixtures;
    - enforce gate reaction/wall budgets;
    - destroy the namespace after recording hashes and ledger references;
    - store admitted artifact as canary unless an existing lifecycle rule permits promoted reuse.

12. **Finish or fail**
    - consume/release name lease atomically;
    - write closed `BuildResult`;
    - deduplicate insufficiencies by normalized need and reason code;
    - retain failed partial structures only as sandbox `debris` with TTL.

### 5.3 Builder invariants

- every `p.*` output immediately enters a deterministic check;
- model output can name only ids offered in its input;
- all shared budgets use atomic decrement-or-fail;
- every backedge has a hard count of one;
- recursive depth and branching are capped independently of model claims;
- a build never mutates an existing artifact version;
- only the gate repository method can create a canary/promoted lifecycle transition.

---

## 6. Reactor: exact live-turn contract

### 6.1 Turn ingress

1. authenticate protocol and tenant;
2. deduplicate message/correlation id;
3. enqueue on the per-instance bounded actor queue;
4. coalesce only according to the tenant's pinned queue policy;
5. hydrate durable rows;
6. compute recency/temporal classifications from the hydrated timestamps;
7. freeze Sense;
8. update `last_seen` only after the snapshot;
9. choose continuation or an already pinned eligible artifact;
10. append `turn_start` and `sense_snapshot` before execution.

### 6.2 Artifact choice

Artifact choice is bounded and explainable:

- an active parked flow has priority;
- only declared read-only detours may interrupt it;
- otherwise the agent ranks already admitted pathway/artifact candidates;
- thresholds and fallback are deterministic after measured scores are available;
- the complete candidate list, scores, margin, entropy, and fallback reason are ledgered;
- no candidate means honest decline plus `CapabilityRequest`, not inline generation.

### 6.3 Step execution

For every planned node:

1. project only declared arguments;
2. validate producer imprint;
3. apply only the pinned edge converter when required;
4. validate consumer imprint;
5. enforce effect policy and target version;
6. execute pure/read/model/tool/sub-artifact behavior;
7. validate declared output imprint;
8. atomically write only the declared write set;
9. append reaction/ledger entries;
10. charge all active budgets.

### 6.4 External effects

```text
policy check
  -> durable call_intent(args_hash, idem_key, deadline)
  -> emit correlated frame
  -> durable call_dispatch(corr)
  -> receive and validate result
  -> durable call_result
  -> continue
```

If no dispatch was recorded, failure is retryable transient. If a write/external call was
dispatched but no result exists, outcome is unknown and non-retryable. Duplicate results are
acknowledged and discarded. Late results are ledgered for triage but never re-enter the flow.

### 6.5 Park and resume

- serialize App I frames, bag, exact flow/kernel pins, event key, and continuation hash;
- commit continuation with the `park` entry;
- authenticate wake by possession of the minted event token or session routing;
- verify hash and pins before reading the payload;
- refresh Sense, recheck policy and Guards, and resume the exact old flow revision;
- garbage-collect a revision only when no continuation or promoted artifact pins it.

### 6.6 Replay

Replay loads the exact program pins, initial bag and ledger. It injects recorded observations and
recomputes pure decisions. Any sequence, nid, kind, args hash, result hash, or final bag mismatch is
a hard diagnostic failure. Replay never silently resynchronizes and never calls providers/tools.

---

## 7. Learning, steward, and controlled growth

### 7.1 Evidence collection

Every learned artifact use records:

- artifact/version and lifecycle status at use;
- distinct input hash;
- prompt/template/model/embedding pins involved;
- consumer nid and downstream effect reach;
- digest validation result;
- same-turn error cause chain and Guard results;
- explicit correction or abandonment signal when available.

Raw repeated traffic cannot inflate promotion thresholds because thresholds use distinct hashes.

### 7.2 Steward responsibilities

The steward is deterministic and may only propose transitions:

- dependency departure: calculate and enqueue demotion/re-gate cascade;
- dependency arrival: match open insufficiencies and enqueue rebuild proposals;
- threshold reached: create a promotion proposal;
- drift/failure threshold crossed: request immediate demotion;
- unused/three-demotion policy reached: request retirement;
- expired lease, debris, sandbox namespace, and continuation: enqueue cleanup.

The lifecycle repository verifies the transition and commits it. The steward never directly writes
`promoted`.

### 7.3 Demand loop

`CapabilityRequest` is a closed record, not a copied conversation transcript. It contains the
normalized need, declared available inputs/imprints, desired output imprint, allowed effects,
tenant, requester, and redacted evidence references. Deduplicate it by `(tenant,need_hash,
reason_code)`, increment demand count, and prioritize by count and tenant policy.

### 7.4 Model and embedding migration

- a new model/embedding id never overwrites an old pin;
- embedding migration writes new vectors before search switches spaces;
- artifacts using a deprecated prompt/model receive re-gate proposals over the same body;
- failed re-gate demotes and creates a rebuild demand;
- similarity values from different embedding spaces are never compared.

---

## 8. Public and internal APIs

### 8.1 Rust control plane

Required authenticated endpoints:

```text
POST /v1/artifacts                       push immutable artifact version
GET  /v1/artifacts/:id[@version]         inspect body, pins, trust and evidence
POST /v1/artifacts/:id/lifecycle         approve, demote, retire
POST /v1/builds                          submit BuildSpec
GET  /v1/builds/:id                      durable build result/progress
POST /v1/flows                           compatibility push, lowers to Artifact
POST /v1/mint                            mint prompt artifact through root
POST /v1/prism                           execute closed database query
GET  /v1/instances/:id/ledger            authorized audit
POST /v1/instances/:id/replay            start bounded replay job
GET  /v1/metrics                         tenant-authorized metrics
```

Long build and replay work returns a job id; it must not occupy an HTTP request for its full life.

### 8.2 Rust data plane

Use `aelio-wire` frames for:

- turn submit/result/error;
- model call/result;
- embedding call/result;
- tool call/result;
- channel delivery/result;
- wake events;
- ack, heartbeat, drain, and protocol negotiation.

Every frame includes protocol major/minor, correlation id, tenant, deadline, and payload imprint or
frame-kind schema. Unknown majors and unknown fields fail closed.

### 8.3 TypeScript restrictions

Add an architectural test that scans the production server dependency graph and source imports.
Production TypeScript must not import the legacy turn executor, pathway selector, promotion gate,
effect authorizer, or durable memory mutation modules. Compatibility libraries may remain outside
the production graph until a separate removal release.

---

## 9. Implementation program

Each phase is a vertical slice. A phase is complete only when its exit tests pass in CI and the
traceability table is updated. Do not start by rewriting working kernel or database primitives.

### Phase 0 — Ratify convergence and repair specifications

Tasks:

- record C-004 Prism amendment in the mother Decision Log;
- correct Technical Mother canonicalization, replay, lifecycle, paths, Sunjet name, and Prism shape;
- remove the duplicate unresolved §14 attack list;
- mark F2/F3/F4/F5/F6/F10/F11/F14 complete and link their files;
- update `FLAGS.md`: close resolved F-011/F-012 and retain genuinely open findings;
- add a machine-readable requirement index, `docs/requirements/aelio.json`.

Exit tests:

- no normative document contains `Sunjet`;
- conflict-lint script finds no conflicting canonicalization/replay/lifecycle constants;
- every requirement id maps to an owner and planned test.

### Phase 1 — Unified artifact model and repositories

Tasks:

- add closed Rust types for `Artifact`, `BuildSpec`, `BuildResult`, pins, trust, examples and
  provenance under `aelio-runtime::artifact`;
- add physical Aelio DB repositories for the collections in §4;
- make `/v1/flows` lower into an artifact record rather than a separate untracked schema;
- migrate Mint prompt storage into the same artifact registry while retaining typed prompt views;
- implement atomic name leases and content-addressed verification cache;
- add explicit schema migrations and version headers.

Exit tests:

- restart preserves artifacts, histories, leases, ledgers and continuations;
- cross-tenant get/search/update attempts fail;
- concurrent identical builds coalesce; conflicting names cannot race;
- artifact identity hash changes exactly when identity fields change;
- no unpinned reference can be stored as executable.

### Phase 2 — Sandbox and generic gate

Tasks:

- implement per-run namespaces and fixture loading;
- run examples through the production reactor with declared stubs;
- enforce reaction/wall/model budgets;
- implement generic lifecycle repository using mother Appendix K;
- persist evidence by distinct input hash;
- make reviewed approval gate first canary consumption;
- integrate converter mutation harness reports.

Exit tests:

- real-tenant records are unreachable from sandbox;
- namespace disappears after success/failure;
- wrong converters are caught and published by phase;
- fabricated, PII, and write-reaching artifacts cannot consume without approval;
- concurrent evidence cannot double-count one input hash.

### Phase 3 — Deterministic builder operations

Tasks:

- implement typed `admit`, `resolve`, `search`, `validate`, `admit_children`, `recurse`, `assemble`,
  `gate`, and `fail` functions;
- implement measured-similarity thresholds and deterministic widening;
- implement interface closure and oscillation checks;
- make every function independently unit-testable without a model;
- persist build state between every transition.

Exit tests:

- adversarial maximum-branch model stubs always terminate within budget;
- invented targets, cycles, unbounded ops, effect escalation and nonliteral paths fail;
- crash/restart at every builder state resumes or fails deterministically;
- no model output reaches a second model/output consumer without a kernel check.

### Phase 4 — Root harness with scripted models

Tasks:

- encode the exact twelve-step state machine;
- install immutable `root` and `root_harness` vendor axioms;
- install seed select/compose/decompose prompts through Mint;
- run the technical mother's self-description consistency test;
- build a simple read-only artifact, a Prism artifact, and a reverse-tool artifact.

Exit tests:

- root harness builds its declared flow-builder example without hand editing;
- every output is a Planner-valid pinned artifact;
- failed atomic capability creates one deduplicated insufficiency;
- artifacts cannot execute in the same turn/job that authored them;
- replay of every sandbox example is identical.

### Phase 5 — Live model authoring under the gate

Tasks:

- use the existing TypeScript/provider boundary for model calls;
- require closed schemas, prompt hashes, model pins, sensitivity projection and one parse retry;
- test refusal/undeterminable routing;
- rate-limit build/model spend per tenant;
- compare live output to scripted conformance cases.

Exit tests:

- malformed, injected, oversized and invented-id outputs cannot produce artifacts;
- secret values never enter prompts or plaintext ledgers;
- provider timeout leaves a resumable/failed durable job, not a stuck lease;
- live build produces the same valid artifact class as the scripted path.

### Phase 6 — Merge the adaptive agent into the artifact model

Tasks:

- define `ArtifactCandidate`, `ArtifactInvocation`, and `CapabilityRequest` contracts;
- adapt agent tier lookup to return pinned artifact references;
- execute selected artifacts only through `aelio-runtime`;
- move effectful agent procedure execution behind runtime Calls;
- make decision log render the shared ledger, not a parallel trace format;
- shadow old/new decisions on the scenario corpus and record parity.

Exit tests:

- no agent code directly dispatches a production tool or mutates flow state;
- greeting, login/OTP, recall, boundary, clarification, interruption and proactive scenarios all
  execute through the Rust reactor;
- one active-flow semantics and read-only detours hold;
- a missing capability creates demand and never an inline ungated procedure;
- old/new parity mismatches are zero or explicitly approved behavior changes.

### Phase 7 — Steward, learning and lifecycle closure

Tasks:

- consume ledger/evidence events into pathway/converter/procedure agreement predicates;
- implement promotion proposals, approvals, demotions, retirement and dependency cascades;
- implement model/template/imprint/kernel version migration rules;
- publish warm-hit, fallback, survival, cost and mutation metrics;
- enforce pre-registered kill criteria.

Exit tests:

- dependency bump triggers the exact required demotion/re-gate cascade;
- repeated identical traffic cannot manufacture promotion evidence;
- Guard violation demotes immediately;
- steward cannot directly promote even with a forged event;
- tenant A evidence never influences tenant B.

### Phase 8 — TypeScript edge cutover

Tasks:

- route widget, WhatsApp, SDK, scheduled and proactive turns through Rust;
- route LLM, embeddings, reverse tools and channel delivery through `aelio-wire`;
- remove TypeScript decision modules from production imports;
- add dedup, late-result, disconnect/reconnect and drain behavior;
- keep credentials only in their designated adapter boundary.

Exit tests:

- killing Rust makes turns fail closed; TypeScript cannot execute a fallback turn;
- duplicate messages/tool results produce one logical effect;
- dispatched unknown-outcome behavior matches effect class;
- protocol-version and tenant spoofing fail closed;
- source/dependency audit proves no TS decision path remains.

### Phase 9 — Production and open-source gate

Tasks:

- run corruption, crash-window, load, soak, fuzz and tenant-isolation suites;
- wire persisted query statistics to remove the remaining linear selectivity count at scale;
- publish benchmark methodology, mutation catch rates and known limitations;
- add migration tooling, backup/restore, readiness, graceful drain and operational runbooks;
- generate public examples and an end-to-end local quickstart.

Exit tests:

- full workspace test and strict Clippy pass;
- live LLM/embedding/tool/channel matrix passes in an opt-in CI environment;
- kill at intent/dispatch/result/park/build/gate commits recovers correctly;
- sustained one-instance and cross-tenant concurrency respects ordering and quotas;
- ledger replay is bit-identical across restart;
- release checklist has no unexplained ignored test or provisional invariant.

---

## 10. Mandatory end-to-end scenarios

Every release runs these through the public boundary and stores the decision log:

1. greeting on cold and warm path;
2. login, invalid phone, OTP send, wrong OTP, correct OTP, restart while parked;
3. duplicate input and duplicate tool result;
4. SDK disconnected before dispatch and after dispatch;
5. read-only question during active flow and a deferred second flow intent;
6. Prism scalar, text, vector, graph and fused recall;
7. unknown field, oversized query, invalid vector and graph-budget rejection;
8. missing capability to insufficiency to scripted build to sandbox canary;
9. reviewed artifact waiting for approval before first consumption;
10. canary promotion from distinct evidence and immediate Guard demotion;
11. prompt/model/embedding/imprint/kernel version changes;
12. cross-tenant row/vector/text/graph/artifact/evidence attacks;
13. replay after database changes without re-running historical reads;
14. late result after unknown outcome;
15. graceful drain with queued turns and active builds.

Each scenario asserts response, final state, effect count, ledger kinds/order, artifact versions,
continuation state, decision-log explanation, and replay result.

---

## 11. Traceability and CI policy

Each normative requirement receives an id such as `SOL-CANON-001`, `KERNEL-POLICY-004`,
`BUILD-GATE-012`, or `WIRE-EFFECT-003`. The machine-readable index records:

```json
{
  "id": "KERNEL-REPLAY-001",
  "source": "AELIO_DSL_MOTHER §12.3",
  "owner": "aelio-kernel",
  "code": ["crates/aelio-kernel/src/ledger.rs"],
  "tests": ["crates/aelio-kernel/tests/nondet_replay.rs"],
  "status": "implemented|partial|missing|external"
}
```

CI gates:

- format and `git diff --check`;
- strict Clippy for all Rust targets;
- full Rust workspace tests;
- property/conformance vectors;
- database corruption and fuzz-view tests;
- architecture dependency/source scan;
- TypeScript typecheck and adapter tests;
- scripted end-to-end scenario matrix;
- opt-in live-provider matrix;
- requirement coverage report with no missing P0/P1 release requirement.

An ignored test must state the external prerequisite and have an explicit CI job that runs when the
prerequisite exists. Ignored tests are never counted as production evidence.

---

## 12. Operational safety rules

- production refuses startup without authentication, event-key secret, durable data directory,
  and required provider/host configuration;
- development open mode is explicit and emits a visible warning;
- request bodies, actor queues, model calls, queries, builds, gates and scans are bounded;
- tenant and principal are derived from authentication, never trusted from body fields alone;
- PII is projected/masked by declared sensitivity; secrets never enter prompts;
- logs render redacted trace data only;
- backup includes catalog, WAL, segments, artifact history, ledger and continuation pins;
- readiness is false during unrecovered migrations or unavailable required storage;
- graceful drain stops admission, completes or parks current work, persists queues, then exits;
- destructive lifecycle actions are audited and recoverable through new versions/history, not row
  deletion.

---

## 13. Definition of fully integrated

The system is fully integrated only when all of the following are true:

- one Rust Planner and Executor govern every executable artifact;
- the adaptive agent invokes pinned artifacts rather than running a competing effectful procedure;
- the complete twelve-step root harness is durable and restartable;
- Mint, flows, procedures, converters, pathways, imprints and datasets share one artifact registry;
- sandbox and generic lifecycle gates govern every model-authored artifact;
- Aelio DB is the durable store for runtime, learning and authoring;
- Prism is the only flexible runtime query language;
- TypeScript performs transport/provider/tool/channel adaptation only;
- every nondeterministic observation required for replay is ledgered;
- Park/restart/resume and effect crash windows are tested through the public boundary;
- tenant isolation holds across every database modality and artifact/evidence lookup;
- no unresolved normative contradiction or production-blocking `PROVISIONAL` remains;
- open-source documentation states measured guarantees and residual risks without claiming semantic
  correctness that the gate cannot prove.

This definition is intentionally stricter than “the crates compile.” It describes one operating
system whose intelligence can grow while its authority remains closed, testable, and auditable.

---

## 14. Immediate next implementation slice

Begin with **Phase 0**, then implement **Phase 1 as one vertical artifact slice**:

```text
BuildSpec fixture
  -> admit + name lease
  -> store immutable artifact candidate
  -> sandbox example through existing runtime
  -> lifecycle transition to canary
  -> retrieve through Prism
  -> invoke pinned flow through /v1/turns
  -> inspect ledger
  -> restart server
  -> replay with identical bag hash
```

Do not begin with live recursive model building. Prove that one manually supplied BuildSpec becomes
one durable, gated, discoverable, executable and replayable artifact using the current kernel and
Aelio DB. Once this slice is green, the twelve-step builder can safely automate how that artifact
is authored without changing how it is trusted or executed.
