# Aelio Conductor and Harness OS: Production Execution Plan

**Status:** Target architecture and implementation plan  
**Priority:** Converge the current system onto a harness-first agent operating system  
**Audience:** Runtime, kernel, database, edge, SDK, safety, testing, and operations engineers  
**Primary outcome:** Every agent behavior that can safely be expressed as a program is a stored, versioned, admitted, replayable Sol program. A root Conductor harness receives events and coordinates other harnesses. Rust remains the trusted kernel; TypeScript remains the channel/model/tool host edge; Aelio DB remains the durable substrate.

---

## 1. Executive decision

Aelio will use the following operating model:

| OS concept | Aelio concept |
|---|---|
| Source/instruction language | Sol plus authoring sugar |
| Program | Immutable Flow/Harness artifact |
| Process | Harness instance |
| Shell / init process | Conductor harness |
| Kernel | `aelio-kernel` plus authoritative runtime admission and dispatch |
| Process-local memory | Bag and scoped harness pages |
| Durable filesystem/data substrate | Aelio DB |
| System calls | Registered `Call` targets |
| Device/service adapters | Model, embedding, SDK tool, channel, clock, and storage targets |
| Suspended process image | Park continuation |
| Execution journal | Hash-chained ledger |
| Standard library | Vendor-signed default harness and target catalog |

The Conductor is itself a pinned, stored Sol program. It is logically long-lived but physically event-driven: it runs until it completes work or reaches `Park`, persists a continuation, and resumes on the next admitted event.

The kernel is not programmable policy. It is the small trusted computing base that validates programs, owns budgets, enforces effects and tenant boundaries, records the ledger, dispatches admitted calls, and verifies continuation/replay integrity. Harnesses may request an action; they cannot bypass the kernel.

---

## 2. Non-negotiable target properties

1. **Harness-first execution.** Conversation routing, selection, clarification, tool use, memory workflows, response composition, and proactive handling execute as harnesses rather than as a second hard-coded agent loop.
2. **All executable steps lower to Sol.** Human-friendly sugar is allowed only if it deterministically lowers to the closed canonical Sol AST before admission.
3. **Pinned dependencies.** An admitted program calls exact target or artifact versions. Runtime semantic lookup cannot silently change a running instance.
4. **Explicit contracts.** Every harness has typed input, output, effect, error, budget, suspendability, and capability contracts. No-input and no-output programs explicitly use `unit`.
5. **Structured concurrency.** Every child belongs to a parent scope. Join, cancellation, failure propagation, and parallel merge behavior are deterministic.
6. **Scoped state.** Child instances receive projected inputs and local pages. Parent/shared mutation occurs only through declared merges or governed storage calls.
7. **Event-driven persistence.** A logically persistent Conductor never depends on an in-memory thread surviving. `Park` produces a durable, tamper-evident continuation.
8. **Replayability.** Completed executions reproduce the same final bag hash from their pinned program and ledger. External results and nondeterminism are injected during replay.
9. **Effect safety.** Writes and external effects require policy admission, intent ledgering, idempotency, and confirmation when required.
10. **Production operability.** The system includes overload control, migration/rollback, observability, tenant isolation, backups, disaster recovery, and measurable SLOs.

---

## 3. Current architecture: assets to preserve

This plan is a convergence plan, not a rewrite.

### 3.1 Canonical Sol kernel

[`aelio-os/crates/aelio-kernel/src/instr.rs`](../aelio-os/crates/aelio-kernel/src/instr.rs) defines the closed 17 control operations:

`Const`, `Identity`, `Seq`, `Let`, `Branch`, `Loop`, `Try`, `Fallback`, `Guard`, `Budget`, `Timeout`, `Once`, `Park`, `Tee`, `Map`, `Filter`, and `Call`.

The parser rejects unknown operations and fields. Preserve this closed-schema property. Do not make every standard-library function a new kernel opcode. Most functionality belongs behind pure or effect-classified registered `Call` targets, or in sugar lowered to these operations.

Related foundations:

- Planner checks: [`aelio-os/crates/aelio-kernel/src/plan.rs`](../aelio-os/crates/aelio-kernel/src/plan.rs)
- Executor/frame stack: [`aelio-os/crates/aelio-kernel/src/exec.rs`](../aelio-os/crates/aelio-kernel/src/exec.rs)
- Instance, park/resume, replay: [`aelio-os/crates/aelio-kernel/src/driver.rs`](../aelio-os/crates/aelio-kernel/src/driver.rs)
- Durable continuations: [`aelio-os/crates/aelio-kernel/src/continuation.rs`](../aelio-os/crates/aelio-kernel/src/continuation.rs)
- Call registry and effects: [`aelio-os/crates/aelio-kernel/src/registry.rs`](../aelio-os/crates/aelio-kernel/src/registry.rs)
- Bag implementation: [`aelio-os/crates/aelio-kernel/src/bag.rs`](../aelio-os/crates/aelio-kernel/src/bag.rs)
- Ledger: [`aelio-os/crates/aelio-kernel/src/ledger.rs`](../aelio-os/crates/aelio-kernel/src/ledger.rs)
- Wave/dependency analysis: [`aelio-os/crates/aelio-kernel/src/waves.rs`](../aelio-os/crates/aelio-kernel/src/waves.rs)
- Canonical values/hashes/paths: [`aelio-os/crates/aelio-sol/src`](../aelio-os/crates/aelio-sol/src)

### 3.2 Existing harness artifacts and library

[`aelio-os/crates/aelio-runtime/src/artifact/harness.rs`](../aelio-os/crates/aelio-runtime/src/artifact/harness.rs) already defines harness nodes and typed seams with pinned converters. Preserve it as the composition artifact format, but complete its executable lowering and lifecycle.

[`aelio-os/crates/aelio-kernel/src/sol_harness_lib.rs`](../aelio-os/crates/aelio-kernel/src/sol_harness_lib.rs) already stores initial Sol harness contracts, including quick reply, intent, wait, memory, confirmation, nested stacks, fallback, guard, once, and budget examples. Treat this as a prototype seed library, not the production registry. Migrate it out of a large Rust source file into signed/versioned artifacts with generated Rust embedding only for bootstrap.

### 3.3 Current runtime

[`aelio-os/crates/aelio-runtime/src/lib.rs`](../aelio-os/crates/aelio-runtime/src/lib.rs) already provides:

- per-instance actor serialization;
- flow push/admission and artifact execution;
- nested calls;
- baseline conversation flow;
- persistence and continuation reconciliation;
- sandbox/replay infrastructure;
- artifact lifecycle, minting, gating, building, and learning.

The current `baseline_conversation_flow` is a model-call loop followed by `Park`. It is the bridge to replace with the production Conductor artifact.

### 3.4 Current agent-specific path

The TypeScript ingress converges in [`server/src/conversation-turn.ts`](../server/src/conversation-turn.ts), which submits to Rust `/agent/v1/turns`. SDK registration is translated into a Rust catalog in [`server/src/aelio-agent-catalog.ts`](../server/src/aelio-agent-catalog.ts). Rust calls models, embeddings, and SDK tools through [`server/src/routes/aelio-host.ts`](../server/src/routes/aelio-host.ts).

Preserve the authority boundary:

- Rust decides and executes the admitted program.
- TypeScript performs transport and external host calls.
- The SDK executes tenant business logic.

The target is to make `/agent/v1/turns` a thin event envelope into the Conductor, not a separate special-purpose orchestration engine.

### 3.5 Storage and operational edge

[`server/src/app.ts`](../server/src/app.ts) requires Rust and Aelio DB at boot. Inbound/outbound workers in [`server/src/workers`](../server/src/workers) already provide durable delivery behavior. These remain edge facilities. Execution truth, continuations, policies, and artifact pins remain Rust-owned.

The existing capability inventory in [`docs/HARNESS_ATOMIC_OPERATIONS_VOCABULARY.md`](../docs/HARNESS_ATOMIC_OPERATIONS_VOCABULARY.md) is an important input to the standard library. It must be reconciled with the contracts in this plan and not remain only a documentation list.

---

## 4. Target execution model

### 4.1 Event envelope

Every incoming stimulus is normalized into one durable envelope before Conductor execution:

```json
{
  "event_id": "provider-stable-or-generated-id",
  "event_type": "user.message",
  "tenant_id": "tenant",
  "subject_id": "stable-user-id",
  "channel": "web",
  "occurred_at": "RFC3339",
  "causation_id": null,
  "correlation_id": "conversation-or-job-id",
  "payload": { "text": "hello" },
  "identity": { "assurance": "anonymous|verified|service" },
  "delivery": { "source": "widget", "source_message_id": "..." },
  "metadata": {}
}
```

Required event families:

- `user.message`, `user.confirmation`, `user.correction`, `user.cancel`
- `system.notification`, `system.state_changed`, `system.policy_changed`
- `tool.completed`, `tool.failed`, `model.completed`, `model.failed`
- `timer.fired`, `job.retry`, `child.completed`, `child.failed`
- `admin.resume`, `admin.cancel`, `admin.replay_request`

Ingress deduplicates on `(tenant_id, source, source_message_id)` or a canonical fallback hash. The normalized event is persisted before acknowledgment for durable channels.

### 4.2 Conductor action contract

The Conductor must produce exactly one validated decision per scheduling cycle:

```text
QuickReply { response_plan }
Spawn { harness_pin, input, join_policy, priority }
SpawnMany { children[], join_policy, merge_plan }
Resume { instance_id, wake }
Continue { instance_id }
Suspend { wait_condition }
Cancel { instance_id, reason }
Escape { scope, condition }
CloseConversation { reason }
Ignore { reason }
DeadLetter { reason, diagnostics }
```

The decision may be informed by an LLM, embeddings, and memory, but the scheduler validates it against the eligible catalog, current lifecycle state, permissions, budgets, and active instance tree.

### 4.3 Harness contract v1

Every executable artifact must declare:

```yaml
id: math.average
version: 1.0.0
artifact_hash: blake3:...
origin: vendor
input_imprint: aelio.math.number_list@1
output_imprint: aelio.math.number@1
errors:
  - Math.EmptyInput
  - Math.DivideByZero
effect: pure
determinism: deterministic
suspendability: never
children:
  allow:
    - math.sum@^1
    - collection.count@^1
    - math.divide@^1
budgets:
  steps: 100
  calls: 3
  wall_ms: 1000
  depth: 4
  fanout: 4
program: {...canonical Sol...}
```

Also record description, tags, trigger surface, examples, authoring source, lowered canonical program, required capabilities, data classification, idempotency strategy, prompt pins, target pins, migration compatibility, test vector hashes, and signature/provenance.

### 4.4 Instance tree and structured concurrency

Each process record contains:

- `instance_id`, `root_instance_id`, `parent_instance_id`;
- tenant, subject, artifact pin, program counter/frame stack;
- state: `ready|running|waiting|completed|failed|cancelled|dead_lettered`;
- local bag root and hashes;
- child set and join policy;
- budgets consumed/remaining;
- continuation pin and wake predicate;
- result/error envelope;
- causation/correlation IDs and ledger head.

Supported join policies:

- `all`: succeed when every child succeeds; deterministic ordered result map;
- `all_settled`: return all success/error envelopes;
- `any`: first successful child wins; remaining children are cancelled;
- `race`: first terminal child wins;
- `quorum(n)`: complete after `n` successes;
- `supervise`: parent continues and receives child events without blocking.

Parallel writes never merge implicitly. `SpawnMany` declares a merge plan and conflict policy. Default conflict policy is reject.

### 4.5 Return, escape, cancel, and terminate

- **Return:** complete current instance with typed output to its direct parent.
- **Error:** complete current instance unsuccessfully with structured `ErrV1`.
- **Escape:** propagate a typed condition to a named or root scope; ancestors declare handlers.
- **Cancel:** cooperative cancellation with a deadline, then terminal cancellation; no new effects after cancellation intent.
- **Terminate root:** privileged Conductor/kernel operation only.

Do not overload `Call` errors to approximate all four behaviors. If canonical Sol lacks sufficient structured semantics, add sugar lowered into existing `Try`/error behavior where possible; propose a normative kernel extension only when lowering cannot preserve behavior and replay.

### 4.6 Bag and page model

Use a namespaced bag convention first; avoid a second state engine:

```text
sys.*               kernel-owned, harness read-only
event.*             current normalized event
context.*           projected runtime context
local.*             current instance page
children.<nid>.*    immutable completed child envelopes
shared.*            explicit root shared state, CAS/merge only
output.*            declared result
error.*             structured error data
sense.*             runtime-owned and never writable by tenant programs
```

Child spawn takes an explicit projection expression. Child completion returns a typed envelope. Parent merge is an explicit operation. Sensitive paths carry classification metadata and are redacted in traces according to policy.

### 4.7 Prompt/model execution

Prompts are immutable database artifacts containing template/version, slots, layers, output schema/imprint, model capability pin, parameter bounds, injection policy, and composed hash. A model call is valid only after prompt composition and must carry `prompt_hash`, as already enforced in the kernel driver.

Model output is untrusted. Every structured model result passes parsing, schema validation, allowed-value validation, and policy validation before it can influence spawn or effects. Free text can only enter customer output through a response/render contract.

---

## 5. Standard harness library

### 5.1 Library rules

The default library is essential product infrastructure, not sample code.

Every entry below must have:

- a stable namespaced ID and semantic version;
- typed input/output imprints and documented errors;
- declared effects, determinism, bounds, and suspendability;
- canonical Sol plus readable authoring form where useful;
- unit, conformance, property, adversarial, and replay tests as applicable;
- benchmark thresholds;
- vendor signature and promotion record;
- examples and composition guidance;
- no hidden sub-plan or undeclared call.

Use four implementation kinds:

- **Compute target:** small pure bounded primitive behind registered `Call`.
- **Kernel composition:** canonical Sol using the 17 control ops.
- **System harness:** composed versioned program.
- **Host target:** effectful external adapter fulfilled through the TypeScript boundary.

### 5.2 Bootstrap and orchestration programs

These are P0 because the OS cannot coherently compose work without them:

| ID | Purpose |
|---|---|
| `conductor.root` | Root event loop and action dispatcher |
| `conductor.decide` | Decide quick reply/spawn/resume/continue/close/ignore |
| `harness.select` | Retrieve, rank, policy-filter, and pin an eligible harness |
| `harness.resolve_pin` | Resolve a version constraint before execution |
| `harness.spawn` | Validate input and create one owned child instance |
| `harness.spawn_many` | Create bounded parallel children atomically |
| `harness.await` | Wait for one child terminal event |
| `harness.join_all` | Deterministic all-child join |
| `harness.join_any` | First successful child with cancellation |
| `harness.join_settled` | Collect all result envelopes |
| `harness.join_quorum` | Bounded quorum join |
| `harness.continue` | Continue a ready instance |
| `harness.resume` | Validate wake and resume a parked instance |
| `harness.suspend` | Persist wait condition/continuation |
| `harness.return` | Validate/export typed child output |
| `harness.escape` | Raise typed condition to an ancestor scope |
| `harness.cancel` | Cancel a child subtree |
| `harness.timeout` | Apply deadline and timeout mapping |
| `harness.retry` | Bounded retry with classified errors/backoff |
| `harness.fallback` | Ordered fallback with accepted error classes |
| `harness.map` | Bounded child program over a collection |
| `harness.filter` | Bounded predicate program over a collection |
| `harness.reduce` | Deterministic fold using a pinned reducer |
| `harness.pipeline` | Sequential typed composition |
| `harness.parallel` | Parallel composition with explicit join/merge |
| `harness.noop` | Unit-to-unit successful program |
| `harness.fail` | Produce a deliberate typed error |
| `harness.inspect` | Return safe instance status/read model |
| `harness.cancel_stale_children` | Reconcile abandoned owned work |

`harness.spawn`, `resume`, and cancellation are privileged system harnesses backed by runtime syscalls; they are not ordinary tenant-writeable target calls.

### 5.3 Core logic and control

`logic.equals`, `logic.not_equals`, `logic.gt`, `logic.gte`, `logic.lt`, `logic.lte`, `logic.and`, `logic.or`, `logic.not`, `logic.xor`, `logic.all`, `logic.any`, `logic.none`, `logic.implies`, `logic.coalesce`, `logic.choose`, `logic.choose_best`, `logic.tie_break`, `logic.assert`, `logic.require_present`, `logic.require_shape`, `logic.constraint_satisfied`, `logic.in_set`, `logic.between`, `logic.is_null`, `logic.is_empty`, `logic.is_truthy`, `logic.compare_versions`.

Composition programs: `control.if_else`, `control.switch`, `control.guard`, `control.try_catch`, `control.try_finally`, `control.fallback`, `control.once`, `control.budget`, `control.timeout`, `control.loop_bounded`, `control.for_each`, `control.while_bounded`, `control.tee`, `control.wait_event`, `control.wait_until`, `control.wait_ttl`.

### 5.4 Arithmetic and statistics

`math.add`, `math.subtract`, `math.multiply`, `math.divide`, `math.modulo`, `math.power`, `math.sqrt`, `math.abs`, `math.negate`, `math.increment`, `math.decrement`, `math.min`, `math.max`, `math.clamp`, `math.round`, `math.floor`, `math.ceil`, `math.truncate`, `math.sign`, `math.percent`, `math.ratio`, `math.normalize_0_1`, `math.lerp`, `math.sum`, `math.product`, `math.average`, `math.weighted_average`, `math.median`, `math.mode`, `math.range`, `math.variance`, `math.stddev`, `math.percentile`, `math.margin`, `math.threshold`, `math.decay`, `math.running_total`, `math.safe_divide`, `math.compare`, `math.random_bounded`.

`math.random_bounded` is nondeterministic and must ledger its result. Arithmetic defines overflow, divide-by-zero, NaN, infinity, integer/float distinction, precision, and rounding semantics in its contract.

Reference composition:

```text
math.average
  SpawnMany [math.sum(values), collection.count(values)] join=all
  Guard count > 0
  Call math.divide(sum, count)
  Return number
```

### 5.5 Collections and structured data

`collection.count`, `collection.first`, `collection.last`, `collection.nth`, `collection.append`, `collection.prepend`, `collection.concat`, `collection.flatten`, `collection.chunk`, `collection.slice`, `collection.take`, `collection.drop`, `collection.reverse`, `collection.sort`, `collection.sort_by`, `collection.unique`, `collection.dedupe_by`, `collection.contains`, `collection.index_of`, `collection.group_by`, `collection.partition`, `collection.zip`, `collection.unzip`, `collection.keys`, `collection.values`, `collection.entries`, `collection.intersection`, `collection.union`, `collection.difference`, `collection.cartesian_bounded`, `collection.sample_bounded`.

`data.parse_json`, `data.serialize_json`, `data.validate_schema`, `data.coerce`, `data.project`, `data.omit`, `data.pick`, `data.get_path`, `data.set_path`, `data.remove_path`, `data.rename_key`, `data.merge`, `data.merge_deep_bounded`, `data.diff`, `data.patch`, `data.default`, `data.canonicalize`, `data.hash`, `data.structural_imprint`, `data.type_of`, `data.is_shape`, `data.csv_parse`, `data.csv_serialize`.

### 5.6 Text, tokens, and language

`text.normalize`, `text.trim`, `text.lowercase`, `text.uppercase`, `text.titlecase`, `text.concat`, `text.join`, `text.split`, `text.lines`, `text.words`, `text.graphemes`, `text.sentence_split`, `text.clause_split`, `text.tokenize`, `text.detokenize`, `text.length`, `text.byte_length`, `text.substring`, `text.replace`, `text.replace_all`, `text.contains`, `text.starts_with`, `text.ends_with`, `text.index_of`, `text.regex_match`, `text.regex_extract`, `text.keyword_extract`, `text.quote_span`, `text.template_fill`, `text.escape_html`, `text.escape_json`, `text.slugify`, `text.truncate`, `text.redact`, `text.mask`, `text.fingerprint`, `text.similarity_lexical`, `text.detect_identifier`, `text.parse_number`, `text.parse_boolean`.

`language.detect`, `language.detect_mixed`, `language.translate`, `language.transliterate`, `language.speech_act`, `language.tone`, `language.ambiguity`, `language.readability`, `language.locale_normalize`, `language.reply_locale`.

LLM-backed language programs must have deterministic fallback or a typed unavailable error; deterministic text functions must not call a model.

### 5.7 Time, identifiers, and scheduling

`time.now`, `time.parse_absolute`, `time.parse_relative`, `time.format`, `time.to_utc`, `time.to_zone`, `time.compare`, `time.add`, `time.subtract`, `time.duration`, `time.age`, `time.range`, `time.range_contains`, `time.overlaps`, `time.bucket`, `time.floor`, `time.ceil`, `time.expired`, `time.recency_weight`, `time.next_schedule`, `time.business_window`, `time.quiet_hours`, `time.backoff`.

`id.uuid`, `id.ulid`, `id.hash`, `id.correlation`, `id.idempotency_key`, `id.canonical_tool`, `id.validate`, `id.namespace`, `id.stable_subject`.

Clock and generated-ID results are ledgered nondeterminism.

### 5.8 Search, embeddings, graph, and evidence

`embedding.create`, `embedding.batch`, `embedding.cache_get`, `embedding.cache_put`, `embedding.dimension_check`, `vector.normalize`, `vector.cosine`, `vector.dot`, `vector.distance`, `vector.centroid`, `vector.nearest`, `vector.cluster_bounded`, `vector.drift`.

`search.exact`, `search.prefix`, `search.lexical`, `search.vector`, `search.semantic`, `search.hybrid`, `search.filter`, `search.limit`, `search.rank`, `search.rerank`, `search.hydrate`, `search.dedupe`, `search.explain`, `search.page`, `search.cursor_validate`.

`graph.node_get`, `graph.node_upsert`, `graph.edge_get`, `graph.edge_upsert`, `graph.neighbors`, `graph.traverse_bounded`, `graph.path_score`, `graph.shortest_path_bounded`, `graph.subgraph`, `graph.remove_edge`, `graph.tombstone_node`.

`evidence.collect`, `evidence.normalize`, `evidence.dedupe`, `evidence.rank`, `evidence.require_minimum`, `evidence.cite`, `evidence.provenance`, `evidence.conflict_detect`, `evidence.recency_check`, `evidence.bundle`.

### 5.9 Memory and context

`memory.write_fact`, `memory.write_event`, `memory.write_preference`, `memory.read_exact`, `memory.recall_semantic`, `memory.recall_recent`, `memory.recall_by_type`, `memory.update`, `memory.merge`, `memory.delete`, `memory.tombstone`, `memory.expire`, `memory.compact`, `memory.summarize`, `memory.conflict_detect`, `memory.confidence_update`, `memory.provenance`, `memory.attach_context`, `memory.detach_context`, `memory.export_subject`, `memory.forget_subject`.

`context.immediate`, `context.window`, `context.bucket`, `context.condense`, `context.select_relevant`, `context.fit_budget`, `context.merge`, `context.redact`, `context.channel_bridge`, `context.active_instances`, `context.suspended_instances`, `context.build_conductor_view`.

Every durable memory write must declare retention, subject, source evidence, confidence, sensitivity, and policy basis. Memory deletion/forgetting is effectful, idempotent, and auditable.

### 5.10 Model and reasoning programs

`model.complete_text`, `model.complete_structured`, `model.classify`, `model.extract`, `model.rank`, `model.route`, `model.disambiguate`, `model.summarize`, `model.translate`, `model.sanitize`, `model.critique`, `model.repair_shape`, `model.compose_reply`, `model.plan_candidate`, `model.check_sufficiency`, `model.infer_slots`, `model.detect_conflict`, `model.explain_decision`.

System harnesses: `reason.intent`, `reason.pathway`, `reason.task_decompose`, `reason.select_harness`, `reason.fill_slots`, `reason.ask_clarification`, `reason.validate_plan`, `reason.replan`, `reason.answer_from_context`, `reason.no_capability_reply`.

No model program may directly create an effect. It returns an untrusted proposal that a symbolic validation harness and kernel policy gate must admit.

### 5.11 Tools and external effects

`tool.discover`, `tool.resolve`, `tool.validate_args`, `tool.policy_check`, `tool.confirm_required`, `tool.dry_run`, `tool.invoke_read`, `tool.invoke_write`, `tool.invoke_external`, `tool.normalize_result`, `tool.classify_error`, `tool.retry_safe`, `tool.compensate`, `tool.audit`, `tool.result_to_evidence`, `tool.unavailable_reply`.

`effect.prepare`, `effect.confirm`, `effect.intent_record`, `effect.execute_once`, `effect.verify`, `effect.compensate`, `effect.result_record`, `effect.notify`, `effect.reconcile_unknown`.

An unknown outcome after external dispatch must never be reported as success. It enters reconciliation using the original idempotency/correlation key.

### 5.12 Conversation and communication

`conversation.quick_reply`, `conversation.greet`, `conversation.thank`, `conversation.farewell`, `conversation.apologize`, `conversation.clarify`, `conversation.confirm`, `conversation.refuse`, `conversation.defer`, `conversation.resume`, `conversation.change_topic`, `conversation.hold_topic`, `conversation.close`, `conversation.no_answer`, `conversation.capability_list`, `conversation.status_update`, `conversation.interruption`, `conversation.correction`, `conversation.cancel_request`.

`reply.plan`, `reply.compose`, `reply.validate_claims`, `reply.validate_policy`, `reply.render`, `reply.flatten`, `reply.localize`, `reply.fit_channel`, `reply.add_citations`, `reply.send`, `reply.delivery_track`, `reply.patch`, `reply.replace_turn`.

`channel.send`, `channel.send_text`, `channel.send_render`, `channel.send_template`, `channel.typing`, `channel.receipt`, `channel.capabilities`, `channel.normalize_address`, `channel.validate_destination`, `channel.retry_delivery`.

### 5.13 Identity, state, policy, and safety

`identity.resolve`, `identity.verify`, `identity.link_channel`, `identity.assurance`, `identity.require_level`, `identity.stable_subject`, `identity.detect_mismatch`, `identity.merge_approved`.

`state.get`, `state.transition`, `state.require`, `state.enter`, `state.exit`, `state.timeout`, `state.history`, `state.active_flow`, `state.set_slot`, `state.clear_slot`.

`policy.evaluate`, `policy.allow`, `policy.deny`, `policy.require_confirmation`, `policy.permission_envelope`, `policy.rate_limit`, `policy.cadence`, `policy.quiet_hours`, `policy.data_access`, `policy.retention`, `policy.tool_access`, `policy.model_access`, `policy.explain`.

`safety.input_scan`, `safety.prompt_injection`, `safety.output_scan`, `safety.pii_detect`, `safety.redact`, `safety.secret_block`, `safety.claim_check`, `safety.effect_check`, `safety.content_policy`, `safety.escalate_human`, `safety.fail_closed`.

### 5.14 Storage, jobs, observability, and operations

`store.get`, `store.get_version`, `store.put`, `store.put_if_absent`, `store.compare_and_swap`, `store.delete`, `store.tombstone`, `store.scan_prefix`, `store.query`, `store.transaction`, `store.lock`, `store.unlock`, `store.dedupe_claim`, `store.checkpoint`, `store.integrity_check`.

`job.enqueue`, `job.schedule`, `job.lease`, `job.renew`, `job.complete`, `job.fail`, `job.retry`, `job.cancel`, `job.dead_letter`, `job.reconcile`, `job.inspect`.

`trace.start`, `trace.step`, `trace.decision`, `trace.call`, `trace.error`, `trace.finish`, `trace.link_child`, `trace.redact`, `trace.explain`, `trace.reconstruct`, `metric.increment`, `metric.observe_latency`, `metric.observe_cost`, `metric.observe_tokens`, `metric.gauge`, `audit.append`.

Storage primitives are not automatically exposed to tenant harnesses. System harnesses receive narrowly scoped capabilities and tenant-bound keys.

### 5.15 Learning and artifact lifecycle

`learn.extract_fact`, `learn.extract_preference`, `learn.extract_aspect`, `learn.observe_pathway`, `learn.observe_procedure`, `learn.update_confidence`, `learn.aggregate_evidence`, `learn.detect_drift`, `learn.create_candidate`, `learn.shadow_test`, `learn.compare_baseline`, `learn.promote`, `learn.reject`, `learn.retire`, `learn.rollback`, `learn.explain`.

`artifact.create_draft`, `artifact.validate`, `artifact.lower`, `artifact.resolve_dependencies`, `artifact.sandbox`, `artifact.property_test`, `artifact.sign`, `artifact.promote`, `artifact.pin`, `artifact.fetch`, `artifact.deprecate`, `artifact.retire`, `artifact.rollback`, `artifact.diff`, `artifact.provenance`.

Learned artifacts never self-promote based only on their own model judgment. Promotion requires fixed evidence gates, sandbox success, policy review, and immutable artifact creation.

### 5.16 Reference compound programs

Ship these composed programs in addition to primitives:

- `workflow.average`, `workflow.weighted_average`, `workflow.parallel_research`, `workflow.retrieve_then_answer`;
- `workflow.identify_then_execute`, `workflow.confirm_then_act`, `workflow.ask_wait_resume`;
- `workflow.tool_read_reply`, `workflow.tool_write_confirm_reply`, `workflow.tool_failure_recover`;
- `workflow.memory_recall_reply`, `workflow.post_turn_memory`, `workflow.session_compact`;
- `workflow.intent_select_spawn`, `workflow.slot_fill_execute`, `workflow.interruption_resume`;
- `workflow.proactive_evaluate_send`, `workflow.notification_route`, `workflow.human_handoff`;
- `workflow.multi_tool_parallel`, `workflow.multi_tool_series`, `workflow.map_tool_bounded`;
- `workflow.plan_validate_execute`, `workflow.answer_or_clarify`, `workflow.no_capability`;
- `workflow.child_escape_handler`, `workflow.child_timeout_fallback`, `workflow.saga_compensate`.

---

## 6. Conductor design

### 6.1 Deterministic outer loop

The root program performs:

1. Validate and deduplicate event.
2. Load active/suspended instance summary, lifecycle state, and policy envelope.
3. Detect a direct deterministic route: confirmation, cancellation, known child completion, timer wake, generic greeting/thanks/farewell.
4. Build a bounded Conductor context view.
5. If no deterministic route exists, run semantic candidate retrieval and optionally a structured model routing call.
6. Validate the proposed action.
7. Execute exactly one scheduling transition atomically.
8. Run/continue eligible child work within the turn budget.
9. Compose and validate zero or more outbound replies.
10. Persist ledger, state, and continuation before returning delivery work.

### 6.2 Selection pipeline

`harness.select` uses staged narrowing:

```text
catalog scope
  -> tenant/state/capability/policy hard filter
  -> trigger/exact match
  -> lexical/vector candidate retrieval
  -> deterministic score combination
  -> optional model disambiguation over top K
  -> margin/sufficiency check
  -> exact immutable pin
  -> input compatibility check
```

If confidence or input sufficiency is inadequate, select `conversation.clarify` rather than guessing. The selected pin and evidence are ledgered.

### 6.3 Active work and interruptions

Define explicit policies:

- A confirmation/correction resumes its matching waiting instance.
- A cancellation targets the referenced or most recent cancellable instance.
- A related answer resumes current work.
- A harmless side question may spawn a supervised child while holding current work.
- A conflicting new task either stacks, replaces, or asks the user, based on tenant policy.
- Maximum active roots, depth, and fanout are bounded per subject and tenant.

### 6.4 Quick reply

Quick reply is still a harness. Deterministic templates handle generic greetings, thanks, and farewells where possible. An LLM-backed quick reply receives a bounded but useful context bundle and must pass claim/policy/render validation. It cannot claim external effects.

---

## 7. Artifact storage and lifecycle

### 7.1 Database records

Create or converge tables/collections for:

- artifact metadata and immutable version bodies;
- canonical programs and authoring sources;
- prompt artifacts and composed hashes;
- imprints, converter pins, target declarations, policies;
- dependency graph and reverse dependency index;
- vendor/tenant signatures and provenance;
- promotion/deprecation/retirement status;
- harness instances, child edges, joins, continuations;
- normalized events, dedupe claims, outbox records;
- ledgers, trace summaries, replay reports;
- library installation manifests and tenant overrides.

Artifact keys are tenant-scoped and immutable after promotion. Updating means creating a new version. Running instances retain old pins.

### 7.2 Standard library packaging

Store authoring artifacts under a repository directory such as:

```text
aelio-os/library/
  manifest.yaml
  imprints/
  targets/
  prompts/
  harnesses/<family>/<id>/<version>/
    contract.yaml
    program.sol.json
    examples/
    vectors/
```

A build tool validates, canonicalizes, hashes, signs, and produces a bootstrap bundle embedded in the Rust server image. On boot, installation is idempotent by `(vendor, id, version, hash)`. Existing promoted versions are never overwritten.

### 7.3 Tenant customization

Tenants may:

- enable/disable eligible vendor harnesses;
- add tenant harnesses and prompts;
- pin approved versions;
- wrap vendor programs with stricter policy;
- override presentation/personality layers where allowed.

Tenants cannot mutate vendor artifacts, loosen kernel policy, shadow reserved IDs, or replace a pinned dependency during execution.

---

## 8. Required kernel/runtime work

### 8.1 Gap audit before implementation

Write executable tests answering these questions before changing semantics:

- Can a registered Flow `Call` another pinned Flow and propagate completion/error correctly?
- Can nested children park and resume through a process restart?
- Are child continuations independently addressable and linked to their parent?
- Does existing wave analysis provide true parallel execution or only dependency analysis?
- Are budgets inherited and subdivided across nested calls?
- Can cancellation prevent post-cancel dispatch?
- Can a parent join multiple durable children deterministically?
- Does replay cover nested, parallel, parked, and effectful trees?
- Can artifact `HarnessBody` lower to an executable Flow graph with seam converters?

Do not assume the large prototype in `sol_harness_lib.rs` proves production nested semantics; promote its tests into independent conformance suites.

### 8.2 Scheduler and process repository

Add a runtime scheduler with atomic transitions, ready queues, per-subject serialization, tenant fairness, priority with starvation bounds, leases for crash recovery, and outbox delivery. A single database transaction should persist scheduling state, ledger head, continuation, and newly created child/outbox records.

### 8.3 Durable child composition

Implement child spawn/join as runtime-owned registered targets or a normative control extension. Requirements:

- deterministic child IDs derived from root/parent/nid/attempt;
- create-if-absent semantics;
- exact artifact pins;
- explicit input projection and output imprint;
- parent ownership and depth/fanout limits;
- durable joins and wake events;
- cancellation propagation;
- deterministic result ordering;
- retry that cannot duplicate children or effects.

### 8.4 Compiler and sugar

Create a compiler pipeline:

```text
authoring YAML/JSON
 -> schema parse
 -> sugar expansion
 -> name resolution
 -> dependency pinning
 -> type/imprint and effect analysis
 -> control/resource analysis
 -> canonical Sol JSON
 -> kernel compile/plan
 -> canonical hash and signed artifact
```

Sugar must preserve source maps from authoring step names to generated `nid`s so traces remain readable. Golden tests assert byte-identical lowering.

### 8.5 Type/imprint checking

Extend static checking across seams and calls. Verify all paths exist where statically knowable, output imprints match destinations, converters are exact pins, nullable/error/unit behavior is explicit, and model/tool results are checked before use.

### 8.6 Resource accounting

Add root and child limits for steps, calls, model tokens, wall time, CPU time where measurable, bag bytes, output bytes, DB reads/writes, search candidates, loop iterations, recursion depth, fanout, active children, and total instances. Children receive sub-budgets; unused budget returns to the parent only under a defined deterministic rule.

---

## 9. Migration from the current agent path

### Stage A: observe and freeze behavior

- Capture current `/agent/v1/turns` scenario matrix, traces, replies, tool calls, confirmation behavior, memory mutations, and latency/cost.
- Define compatibility envelopes rather than exact prose equality.
- Add correlation across TypeScript ingress, Rust turn, child instances, host calls, SDK invocation, and delivery.

### Stage B: build Conductor behind shadow mode

- Install `conductor.root` as a vendor artifact.
- Feed it copies of real/test normalized events with external effects stubbed.
- Compare selected actions and required capabilities with the existing agent path.
- Record disagreements without serving Conductor results.

### Stage C: deterministic routes

Route generic messages, explicit confirmation/cancel, child/timer events, and exact trigger matches through Conductor. Keep complex free-form requests on the old path. Require replay and SLO gates.

### Stage D: selection and quick reply

Enable semantic selection and the bounded quick-reply harness for opted-in tenants. Fall back safely to the current path on pre-effect failures. Never fallback after an external effect may have dispatched.

### Stage E: tool workflows and memory

Move read tools, then confirmed writes, then multi-tool flows, then post-turn memory into harness programs. Reconcile every old specialized decision with a named standard program or an explicitly retained kernel invariant.

### Stage F: authoritative cutover

Change `/agent/v1/turns` to normalize an event and invoke/resume the tenant’s pinned Conductor. Remove the old agent loop as an execution authority. Retain read-only compatibility/admin projections as needed.

### Stage G: cleanup

- Remove duplicate decision logic and obsolete configuration.
- Migrate prototype hard-coded harness contracts into the artifact bundle.
- Update README/manual/SDK documentation and operational runbooks.
- Keep rollback support for one release train, then delete the old executor.

---

## 10. API and edge changes

### Rust API

Add/standardize:

- `POST /agent/v2/events` — durable normalized event submission;
- `GET /agent/v2/instances/:id` — safe process status;
- `POST /agent/v2/instances/:id/cancel`;
- `POST /agent/v2/instances/:id/resume` for privileged recovery;
- artifact/library install, validate, promote, pin, and diff endpoints;
- replay and trace reconstruction endpoints;
- readiness reporting library/catalog/schema compatibility.

Use idempotency keys and explicit conflict responses. Do not expose raw internal bag secrets.

### TypeScript edge

Refactor [`server/src/conversation-turn.ts`](../server/src/conversation-turn.ts) to submit normalized events rather than an agent-specific utterance structure. Keep render-frame validation and text fallback. Preserve the narrow host boundary in [`server/src/routes/aelio-host.ts`](../server/src/routes/aelio-host.ts), extending only with explicitly admitted target families.

SDK catalog registration remains atomic: Rust admits the translated catalog and pinned flow artifacts before [`server/src/sdk-bridge.ts`](../server/src/sdk-bridge.ts) switches the callable connection.

### SDK

Add APIs to register tenant harness source/artifact references, states, policies, tool schemas, and channel capabilities. SDKs do not execute Sol and do not become schedulers. They continue to expose business functions and receive correlated invocations.

---

## 11. Safety and security requirements

- Authenticate and authorize every Rust/TypeScript/SDK boundary with rotated secrets or workload identity.
- Bind every target declaration and storage key to a tenant.
- Use constant-time secret comparison and never place secrets in query strings.
- Treat user, tool, memory, retrieved, and model content as untrusted data.
- Separate instructions from data in prompt composition.
- Enforce model output schemas and allowlists.
- Require idempotency keys for writes/external effects.
- Ledger intent before dispatch and result afterward.
- Require explicit confirmation artifacts for governed effects.
- Encrypt sensitive durable data and backups; redact logs/traces.
- Implement subject export/deletion and retention policies.
- Sign vendor library manifests; verify hashes at boot and admission.
- Reject artifact dependency cycles unless a separately bounded recursion contract explicitly permits them.
- Prevent confused-deputy access by deriving subject/tenant context in the runtime, never accepting it from model output.
- Add SSRF/egress allowlisting and request size/deadline limits at host adapters.
- Rate-limit per tenant, subject, channel, tool, and model budget.
- Fail closed on policy/catalog/signature uncertainty.

Threat-model prompt injection, malicious harness artifacts, dependency substitution, continuation tampering, replay attacks, duplicate webhooks, cross-tenant path access, cancellation races, unknown external outcomes, model schema attacks, resource exhaustion, and poisoned learned artifacts.

---

## 12. Reliability and production operations

### SLO starting targets

| Measure | Initial production target |
|---|---|
| Event acceptance availability | 99.95% monthly |
| No-effect deterministic route p95 | < 150 ms excluding network ingress |
| Scheduler transition p99 | < 250 ms |
| Continuation durability | acknowledged only after durable commit |
| Duplicate external effects | zero for contract-compliant idempotent tools |
| Replay match | 100% bag-hash match for replayable completed instances |
| Cross-tenant data/effect leakage | zero |
| Recovery point objective | <= 5 minutes, configurable |
| Recovery time objective | <= 30 minutes, configurable |

Model/tool latency is reported separately from runtime overhead.

### Operational controls

- health, readiness, dependency, and library-integrity probes;
- graceful drain: stop accepting work, persist leases, finish/park safe turns;
- bounded queues with overload responses and retry-after;
- per-tenant circuit breakers for models/tools/channels;
- dead-letter inspection and controlled replay;
- backup verification through automated restore drills;
- schema migration forward/backward compatibility for rolling deployment;
- canary tenant and percentage rollout for Conductor/library versions;
- instant pin rollback without mutating historical instances;
- dashboards for event lag, ready queue, park age, child fanout, errors, model/tool latency, token/cost, effect reconciliation, and delivery lag;
- alerts on ledger breaks, continuation verification failure, signature failure, replay mismatch, stuck joins, and abnormal effect retries.

---

## 13. Test and verification program

### Per primitive/harness

- contract schema and closed-field rejection;
- success, boundary, typed error, timeout, and budget vectors;
- canonical hash and replay test;
- deterministic property tests where applicable;
- fuzz malformed inputs and hostile sizes;
- effect/idempotency tests;
- prompt injection and malformed model-output tests;
- performance benchmark with regression threshold.

### Kernel/system suites

1. A→B→C nested completion and failure propagation.
2. Parent→parallel children→all/any/quorum joins.
3. Nested child parks, process restarts, wake, and parent continuation.
4. Cancel before dispatch, during call, after unknown outcome, and during join.
5. Root/child budget subdivision and exhaustion.
6. Duplicate event, spawn, resume, tool result, and delivery attempts.
7. Exact version pin across live library upgrade.
8. Replay nested/parallel/parked/effectful instances.
9. Crash injection between every intent/dispatch/result/state/outbox boundary.
10. Tenant isolation across artifact, memory, continuation, trace, and host calls.
11. Conductor interruption, correction, topic switch, and stale continuation behavior.
12. Model unavailable, embedding unavailable, tool unavailable, DB pressure, and channel outage.
13. Large catalog selection accuracy and latency.
14. Learned artifact cannot bypass promotion gates.

### End-to-end acceptance scenarios

- Web greeting produces deterministic quick reply with no embedding/model call where configured.
- User asks order status; Conductor selects a read workflow, invokes SDK, and replies.
- User asks cancellation; system gathers slots, confirms, executes once, and reports result.
- User changes topic during confirmation; original task is held and resumed correctly.
- Average harness runs sum/count in parallel then division.
- System push notification selects a notification harness without pretending to be user input.
- Restart while nested child waits; next event resumes exact pinned stack.
- Duplicate WhatsApp webhook produces one logical turn and no duplicate action.
- Model proposes a nonexistent harness/tool; validator rejects and clarifies/falls back.
- Tenant upgrades library while an old instance is parked; old instance resumes old pin.
- Backup restore reproduces artifact hashes, continuations, and ledger heads.

Release requires `cargo test`, workspace TypeScript tests, scenario tests, Docker verification, migration tests, security suite, replay corpus, and load/soak tests to pass.

---

## 14. Implementation workstreams and phases

### Phase 0 — architecture lock and inventory (2–3 weeks)

- Write normative Conductor, instance-tree, join, cancellation, bag-scope, and harness contract specs.
- Audit current implementation against each invariant.
- Convert uncertainties into explicit flags/decisions; do not silently change locked Sol semantics.
- Build a machine-readable inventory mapping the atomic vocabulary to existing compute targets, prototype harnesses, missing targets, and duplicates.
- Establish baseline behavior, performance, and replay corpus.

**Exit:** approved contracts, gap matrix, no unresolved P0 semantic ambiguity, baseline dashboards/tests.

### Phase 1 — artifact toolchain and library skeleton (3–5 weeks)

- Create on-disk library layout and manifest schema.
- Implement validate/lower/pin/hash/sign/bundle CLI.
- Generate source maps and documentation.
- Migrate existing `sol_harness_lib.rs` contracts without behavior change.
- Implement imprints and first P0 primitive targets.

**Exit:** reproducible signed bundle, idempotent install, exact hash tests, prototype parity.

### Phase 2 — durable process tree (5–8 weeks)

- Implement instance repository, child edges, scheduler transitions, leases, joins, cancellation, budgets, and outbox.
- Complete nested durable park/resume and restart recovery.
- Complete tree replay and trace linking.

**Exit:** crash-injected nested/parallel conformance suite and replay pass.

### Phase 3 — P0 standard library (4–7 weeks, overlaps after contracts lock)

- Bootstrap/orchestration programs.
- Core logic/control, data, math, text, time, IDs.
- Model/prompt validation, tool safety, reply/render programs.
- Memory/context and selection minimum set.

**Exit:** average, retrieve-answer, confirm-act, ask-wait-resume, and interruption scenarios pass exclusively through harnesses.

### Phase 4 — Conductor shadow and deterministic cutover (4–6 weeks)

- Implement normalized events and Conductor root.
- Shadow current traffic/test corpus.
- Cut over deterministic routes and exact trigger matches.
- Add operational dashboards and rollback pins.

**Exit:** parity thresholds, replay 100%, SLOs met in canary.

### Phase 5 — semantic routing, tools, and memory cutover (6–10 weeks)

- Enable retrieval/model-assisted selection.
- Move read tools, confirmed writes, multi-step flows, memory, and proactive work.
- Run adversarial, load, long-lived continuation, and disaster recovery tests.

**Exit:** all supported conversation scenarios run through Conductor; old path is read-only fallback behind an emergency flag.

### Phase 6 — authoritative removal and production hardening (3–5 weeks)

- Remove duplicate orchestration authority.
- Complete full standard library families and developer documentation.
- Conduct security review, capacity test, restore drill, upgrade/rollback drill, and operational game day.

**Exit:** old executor removed, production readiness review signed, no P0/P1 defects.

### Phase 7 — learning and ecosystem (post-cutover)

- Learned candidate generation/shadow/promotion.
- Tenant authoring SDK/CLI and artifact marketplace controls.
- Expanded domain libraries and formal compatibility certification.

---

## 15. Delivery order for the library

Do not attempt hundreds of programs as unverified stubs. Deliver complete vertical slices:

1. `logic`, `control`, `data`, `math`, `collection`.
2. `harness` orchestration and durable process syscalls.
3. `text`, `time`, `id`, prompt/model validation.
4. `artifact`, `search`, `embedding`, selection.
5. `tool`, `effect`, `policy`, `safety`.
6. `reply`, `conversation`, `channel`.
7. `memory`, `context`, state, identity.
8. `job`, proactive, observability, operations.
9. learning and graph programs.

Each slice ships only when its contracts, artifacts, tests, benchmarks, documentation, telemetry, and rollback are complete.

---

## 16. Ownership boundaries

| Area | Authority |
|---|---|
| Sol schema, planning, execution, budgets, ledger, replay | Rust kernel |
| Artifact admission, scheduler, process tree, Conductor | Rust runtime |
| Programs/prompts/imprints/continuations/memory | Aelio DB through Rust-owned repositories |
| LLM and embedding provider calls | TypeScript host, only on Rust request |
| Web/WhatsApp ingress and presentation validation | TypeScript edge |
| Tenant business functions and business database | Convox SDK in tenant backend |
| Tool/effect permission decision | Rust policy/kernel |
| Delivery adapter | TypeScript/channel SDK, driven by durable outbox |

No component may implement a convenient second version of another component’s authority.

---

## 17. Definition of done

The target architecture is complete only when:

- every customer/system event enters a tenant-pinned Conductor harness;
- Conductor decisions are stored, explainable, policy-checked, and replayable;
- child harnesses execute in nested and parallel durable trees with correct joins/cancellation;
- every executable behavior lowers to canonical Sol or an admitted typed target;
- the default signed library is installed and contains all P0/P1 families in this plan;
- prompts, tools, policies, imprints, and artifact dependencies are exact versioned pins;
- bags/pages are scoped and parallel merges are explicit;
- parked trees survive restart and library upgrades;
- external effects are intent-ledgered, idempotent, confirmable, and reconcilable;
- old specialized conversational orchestration is removed as an authority;
- replay matches final bag hashes across the production replay corpus;
- security, load, soak, crash, migration, backup/restore, and rollback tests pass;
- SLO dashboards and alert/runbook coverage exist;
- documentation lets a developer author, test, publish, pin, execute, inspect, and roll back a harness without reading kernel source.

---

## 18. First concrete engineering backlog

1. Write `HarnessContractV1`, `NormalizedEventV1`, `ConductorActionV1`, `InstanceRecordV1`, `ChildJoinV1`, and `ResultEnvelopeV1` schemas.
2. Add conformance vectors for average parallel composition, nested park/resume, joins, cancellation, and version pinning.
3. Produce a current-gap report from `instr.rs`, `exec.rs`, `driver.rs`, `waves.rs`, runtime artifact handling, and `/agent/v1/turns`.
4. Create the filesystem-backed standard-library layout and deterministic bundle builder.
5. Migrate the existing seed contracts from `sol_harness_lib.rs` into artifacts.
6. Implement the P0 compute targets: core logic, arithmetic, collection, data, text, time, and ID primitives.
7. Implement durable child spawn/join/cancel and restart recovery.
8. Implement `harness.select`, `harness.spawn`, `harness.await`, and `harness.join_all`.
9. Implement `workflow.average` as the first canonical parallel reference program.
10. Implement `conductor.root` with deterministic routes only.
11. Add normalized event submission while preserving the existing endpoint as an adapter.
12. Shadow the Conductor against the scenario matrix and real harness observation corpus.
13. Add model-assisted selection behind hard validation.
14. Migrate tool, confirmation, response, and memory workflows.
15. Canary, harden, cut over, and remove the duplicate agent executor.

This order proves the core operating-system claim early: a stored Conductor can reliably select and run stored programs, programs can compose other programs in series or parallel, state survives suspension and restart, and the complete execution remains safe and replayable.
