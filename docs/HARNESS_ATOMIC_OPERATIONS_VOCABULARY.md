# Harness Atomic Operations Vocabulary

This is the first vocabulary pass for the self-learning Aelio DB harness.

An **atomic operation** is one independent capability that can be called, tested,
traced, budgeted, and composed. It should not secretly run an entire plan. A
plan, daemon, workflow, or conversation turn is a composition of atomic
operations.

The goal is to make the harness able to say:

- what it can do
- what each thing needs as input
- what each thing returns
- whether it reads, writes, calls an LLM, calls a tool, or only transforms data
- which operation made each decision shown in admin transparency

## Atomic Operation Contract

Every operation should eventually have this shape:

```ts
type AtomicOperation<I, O> = {
  id: string;
  family: string;
  description: string;
  input: I;
  output: O;
  deterministic: boolean;
  sideEffects: "none" | "read" | "write" | "external";
  timeoutMs?: number;
  traceKind?: string;
  implementation?: string;
};
```

The harness can then compose these operations into higher-level flows:

```text
receive.message
  -> identity.resolve
  -> generic.classify
  -> embedding.create
  -> immediate_context.read
  -> pathway.decide
  -> archetype.assess
  -> prompt.compile
  -> llm.plan
  -> tool.execute
  -> llm.reply
  -> trace.write
```

## Operational Capability Atoms

This is the deeper vocabulary layer: the tiny independent things the harness can
do before those things are composed into larger operations.

For example, `time.now` is not a whole conversation feature. It is a capability
atom. Once the harness has atoms like `time.now`, `time.bucket`, `text.split`,
`embedding.create`, `query.build`, `search.vector`, `score.compare`,
`policy.check`, and `reply.send`, it can build larger operations such as
"immediate context engine", "semantic retrieval", "proactive daemon", or
"admin decision explanation".

This v0 capability layer defines **170 small capability atoms** across **18
capability groups**.

| Capability group | Count | What it lets the harness do |
| --- | ---: | --- |
| Time | 10 | Understand now, ranges, buckets, expiry, schedules, and recency |
| Text | 12 | Normalize, split, match, summarize, quote, and extract text |
| Language | 6 | Detect language, tone, speech act, and ambiguity |
| Math | 10 | Count, compare, aggregate, normalize, threshold, and calculate |
| Logic | 8 | Branch, assert, negate, satisfy constraints, and choose |
| Structured Data | 10 | Parse, validate, coerce, project, diff, merge, and serialize objects |
| Embedding/Vector | 10 | Embed, compare vectors, cluster, centroid, and detect drift |
| Search/Retrieval | 12 | Search text/vector/hybrid, filter, rank, rerank, and hydrate evidence |
| Graph | 8 | Traverse relationships and connect users, memories, aspects, tools, policies |
| Memory/Context | 10 | Write, recall, summarize, bucket, compact, and expire context |
| State/Workflow | 10 | Start, advance, pause, resume, complete, cancel, and inspect flows |
| Policy/Safety | 9 | Allow, block, redact, confirm, rate-limit, and enforce permissions |
| Tool/External | 10 | Discover, bind, call, retry, normalize, and audit external actions |
| LLM/Reasoning | 9 | Ask bounded questions, classify, plan, replan, critique, and synthesize |
| Communication | 8 | Send, format, translate, confirm, defer, nudge, and close messages |
| Learning | 9 | Extract facts, discover aspects, update confidence, promote, retire |
| Observability | 9 | Trace, log, measure latency/cost, reconstruct, and explain decisions |
| Storage | 10 | Insert, update, delete, scan, transactionally write, lock, and deduplicate |

### Time Capability Atoms

| Atom | Meaning |
| --- | --- |
| `time.now` | Get the current timestamp. |
| `time.parse_absolute` | Parse explicit dates and times. |
| `time.parse_relative` | Convert "today", "tomorrow", "last week", "in 5 minutes" into absolute ranges. |
| `time.bucket` | Put a timestamp into a 5m, 15m, 30m, 1h, 1d, or custom bucket. |
| `time.range_contains` | Check whether a timestamp falls inside a range. |
| `time.compare` | Decide before/after/same-window ordering. |
| `time.age` | Compute how old evidence is. |
| `time.recency_weight` | Convert age into a relevance multiplier. |
| `time.expire` | Decide whether a record has passed retention or cache TTL. |
| `time.schedule` | Compute the next eligible time for a proactive/action job. |

### Text Capability Atoms

| Atom | Meaning |
| --- | --- |
| `text.normalize` | Clean whitespace and canonicalize raw text. |
| `text.tokenize` | Split text into words/tokens for lexical matching. |
| `text.sentence_split` | Split text into sentence spans. |
| `text.clause_split` | Split mixed messages into smaller clauses. |
| `text.substring_match` | Find exact words or phrases. |
| `text.regex_match` | Match deterministic patterns such as ids, emails, order numbers. |
| `text.keyword_extract` | Extract important terms from a span. |
| `text.entity_extract` | Extract names, products, plans, dates, amounts, and ids. |
| `text.quote_span` | Preserve source evidence text with offsets. |
| `text.summarize_short` | Compress a small text block without changing meaning. |
| `text.template_fill` | Fill deterministic response or prompt templates. |
| `text.redact` | Replace sensitive substrings with safe placeholders. |

### Language Capability Atoms

| Atom | Meaning |
| --- | --- |
| `language.detect` | Detect the dominant language. |
| `language.detect_mixed` | Detect multilingual or code-switched text. |
| `language.translate` | Translate text when needed for retrieval or reply. |
| `speech_act.classify` | Classify ask, command, complaint, greeting, thanks, farewell, correction, or refusal. |
| `tone.classify` | Classify tone such as calm, frustrated, confused, urgent, satisfied, disengaged. |
| `ambiguity.detect` | Detect when the message is underspecified or could mean multiple things. |

### Math Capability Atoms

| Atom | Meaning |
| --- | --- |
| `math.count` | Count items, matches, attempts, or events. |
| `math.sum` | Sum numeric values. |
| `math.average` | Compute average value. |
| `math.min_max` | Find minimum and maximum. |
| `math.percent` | Compute ratio or percentage. |
| `math.normalize_0_1` | Normalize arbitrary score into a 0..1 range. |
| `math.threshold` | Check whether a score crosses a threshold. |
| `math.margin` | Compute winner gap between top scores. |
| `math.weighted_sum` | Combine scored signals using weights. |
| `math.decay` | Apply time or confidence decay. |

### Logic Capability Atoms

| Atom | Meaning |
| --- | --- |
| `logic.equals` | Check equality. |
| `logic.compare` | Compare ordered values. |
| `logic.and` | Require all conditions. |
| `logic.or` | Require at least one condition. |
| `logic.not` | Negate a condition. |
| `logic.constraint_satisfied` | Validate a set of requirements. |
| `logic.choose_best` | Pick highest-scoring candidate. |
| `logic.tie_break` | Resolve equal or close candidates deterministically. |

### Structured Data Capability Atoms

| Atom | Meaning |
| --- | --- |
| `data.parse_json` | Parse JSON-like model/tool output. |
| `data.validate_schema` | Check object against a schema. |
| `data.coerce_type` | Convert strings to numbers, booleans, dates, ids, or enums. |
| `data.project` | Select only needed fields. |
| `data.filter` | Keep records matching a condition. |
| `data.sort` | Order records by field or score. |
| `data.group_by` | Group records by key. |
| `data.diff` | Compute before/after changes. |
| `data.merge` | Merge compatible partial objects. |
| `data.serialize` | Convert object into transport/storage format. |

### Embedding And Vector Capability Atoms

| Atom | Meaning |
| --- | --- |
| `embedding.create` | Convert text into a vector. |
| `embedding.batch_create` | Create vectors for multiple spans together. |
| `embedding.cache_lookup` | Reuse vector for identical text/model hash. |
| `embedding.dimension_check` | Verify vector dimensionality. |
| `vector.normalize` | Normalize vector length. |
| `vector.cosine` | Compute cosine similarity. |
| `vector.centroid` | Compute representative vector for a group. |
| `vector.nearest` | Find nearest vectors. |
| `vector.cluster` | Group nearby vectors. |
| `vector.drift_detect` | Detect when new evidence diverges from an existing centroid. |

### Search And Retrieval Capability Atoms

| Atom | Meaning |
| --- | --- |
| `query.build` | Construct a typed query request. |
| `query.filter_apply` | Apply structured filters such as tenant, user, type, and time. |
| `query.limit` | Limit result count. |
| `search.exact` | Retrieve exact id/key matches. |
| `search.lexical` | Retrieve by keyword/BM25-like text match. |
| `search.vector` | Retrieve by vector similarity. |
| `search.semantic` | Retrieve by semantic query intent. |
| `search.hybrid` | Combine lexical, vector, filters, and graph constraints. |
| `rank.initial` | Order raw candidates by native search score. |
| `rank.rerank` | Reorder candidates using final harness criteria. |
| `evidence.hydrate` | Fetch full source rows for top candidate ids. |
| `evidence.dedupe` | Remove duplicate or near-duplicate evidence. |

### Graph Capability Atoms

| Atom | Meaning |
| --- | --- |
| `graph.node_get` | Fetch one graph node. |
| `graph.node_upsert` | Create or update a graph node. |
| `graph.edge_get` | Fetch relation between nodes. |
| `graph.edge_upsert` | Create or update relation between nodes. |
| `graph.neighbors` | Get adjacent nodes. |
| `graph.traverse` | Walk bounded edges from a start node. |
| `graph.path_score` | Score strength of a relationship path. |
| `graph.subgraph_extract` | Pull the small relevant subgraph for one decision. |

### Memory And Context Capability Atoms

| Atom | Meaning |
| --- | --- |
| `context.append` | Add new message or event into current context. |
| `context.bucket_write` | Store context into a time bucket. |
| `context.bucket_rollup` | Move older context into larger buckets. |
| `context.window_read` | Read recent conversation windows. |
| `context.summary_read` | Read compressed older summary. |
| `context.summary_write` | Store compressed summary. |
| `memory.extract_fact` | Extract durable user/business fact. |
| `memory.store_fact` | Persist accepted memory. |
| `memory.recall_fact` | Retrieve relevant durable memory. |
| `memory.expire_or_retire` | Remove stale, invalid, or low-confidence memory. |

### State And Workflow Capability Atoms

| Atom | Meaning |
| --- | --- |
| `state.read` | Read current state. |
| `state.write` | Write new state. |
| `state.transition` | Move state by event and rules. |
| `flow.start` | Start a new flow. |
| `flow.inspect` | Inspect active flow and missing fields. |
| `flow.advance` | Move to the next step. |
| `flow.pause` | Pause until user/tool/event. |
| `flow.resume` | Resume paused flow. |
| `flow.complete` | Mark flow complete. |
| `flow.cancel` | Cancel or abandon flow. |

### Policy And Safety Capability Atoms

| Atom | Meaning |
| --- | --- |
| `policy.retrieve` | Pull relevant policy rules. |
| `policy.evaluate` | Decide allow/block/confirm/escalate. |
| `permission.check` | Check whether actor/action is authorized. |
| `confirmation.required` | Decide whether user confirmation is required. |
| `confirmation.verify` | Interpret yes/no/changed user confirmation. |
| `rate_limit.check` | Check tenant/user/tool/model rate limits. |
| `risk.score` | Score business or safety risk. |
| `privacy.redact` | Redact sensitive information. |
| `retention.check` | Enforce record retention and deletion policy. |

### Tool And External Capability Atoms

| Atom | Meaning |
| --- | --- |
| `tool.discover` | Find available tools. |
| `tool.rank` | Rank tools for the current intent. |
| `tool.schema_read` | Read input/output schema. |
| `tool.bind_args` | Bind args from message/context/evidence. |
| `tool.validate_args` | Validate args before execution. |
| `tool.call` | Execute the tool. |
| `tool.retry` | Retry transient tool failure. |
| `tool.result_parse` | Parse tool response. |
| `tool.result_normalize` | Convert result into evidence facts. |
| `tool.audit` | Record tool call/result. |

### LLM And Reasoning Capability Atoms

| Atom | Meaning |
| --- | --- |
| `llm.ask` | Ask a bounded question. |
| `llm.classify` | Classify into known labels. |
| `llm.extract` | Extract structured fields. |
| `llm.plan` | Produce a bounded plan AST. |
| `llm.replan` | Repair a failed/blocked plan. |
| `llm.summarize` | Summarize context. |
| `llm.critique` | Critique an action or reply. |
| `llm.synthesize` | Produce final natural-language response. |
| `llm.self_check` | Check the response against prompt facts/policies. |

### Communication Capability Atoms

| Atom | Meaning |
| --- | --- |
| `reply.compose` | Build final user-visible message. |
| `reply.format_channel` | Adapt message for chat, WhatsApp, email, widget, or admin. |
| `reply.send` | Send the message. |
| `reply.defer` | Tell the user work is continuing or delayed. |
| `reply.confirmation_prompt` | Ask for approval of a pending action. |
| `reply.clarifying_question` | Ask for missing information. |
| `reply.proactive_nudge` | Send a follow-up when allowed. |
| `reply.close` | End gracefully when the user disengages. |

### Learning Capability Atoms

| Atom | Meaning |
| --- | --- |
| `learn.observe` | Record a learning candidate from evidence. |
| `learn.extract_aspect` | Identify recurring trait, preference, concern, or behavior. |
| `learn.attach_evidence` | Attach source span/tool result to the learning candidate. |
| `learn.confidence_update` | Increase/decrease confidence. |
| `learn.promote` | Move candidate into active use. |
| `learn.retire` | Stop using stale or wrong learned knowledge. |
| `learn.merge` | Merge duplicate learned facts/aspects. |
| `learn.feedback_apply` | Apply explicit user/admin feedback. |
| `learn.calibrate_threshold` | Adjust scoring threshold from evaluation set. |

### Observability Capability Atoms

| Atom | Meaning |
| --- | --- |
| `trace.start` | Open a turn/action trace. |
| `trace.step` | Record one operation and its result summary. |
| `trace.score` | Record scoring inputs and output. |
| `trace.prompt` | Record prompt assembly metadata. |
| `trace.reply` | Record final reply metadata. |
| `trace.error` | Record failure. |
| `metric.latency` | Measure operation duration. |
| `metric.cost` | Measure model/tool cost. |
| `admin.explain` | Reconstruct why the harness did what it did. |

### Storage Capability Atoms

| Atom | Meaning |
| --- | --- |
| `store.insert` | Insert record. |
| `store.get` | Fetch record by id/key. |
| `store.update` | Patch record. |
| `store.delete` | Delete record. |
| `store.scan` | Read multiple records. |
| `store.upsert` | Insert or update by key. |
| `store.transaction` | Commit multiple writes together. |
| `store.lock` | Guard concurrent state mutation. |
| `store.dedupe` | Prevent duplicate events/jobs/messages. |
| `store.compact` | Compact or optimize storage. |

These capability atoms are the vocabulary the planner should know. A bigger
operation is valid only if it can be decomposed into these smaller atoms or if a
new atom is explicitly added to this list with input, output, side-effect, and
trace behavior.

## Count

This v0 vocabulary defines **100 atomic operations** across **10 operation
families**.

| Family | Count | Purpose |
| --- | ---: | --- |
| Sense | 11 | Observe incoming user, time, session, identity, and active state |
| Transform | 12 | Convert raw data into normalized text, spans, embeddings, prompts, or args |
| Query | 13 | Build and execute Aelio DB/database queries |
| Retrieve | 9 | Pull relevant context, memories, tools, policies, and prior replies |
| Evaluate | 10 | Score, fuse, validate, gate, and choose |
| Reason | 7 | Ask an LLM for bounded classification, planning, critique, or synthesis |
| Act | 10 | Call tools, advance flows, send replies, and request confirmations |
| Control | 10 | Sequence, branch, loop, retry, suspend, resume, and lock |
| Learn | 7 | Extract, store, discover, promote, and retire learned knowledge |
| Observe/Govern | 11 | Trace, redact, audit, enforce tenant/policy/security constraints |

Status legend:

- `implemented`: present in the current tree
- `partial`: present, but should be made more explicit or generalized
- `planned`: should be added as first-class vocabulary/registry entries

## 1. Sense Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `receive.message` | Accept an inbound user message | channel payload | normalized turn request | implemented |
| `receive.event` | Accept a non-message event such as timeout, delivery, webhook, or daemon tick | event payload | event turn request | partial |
| `time.now` | Read the current clock | none | timestamp | implemented |
| `identity.resolve` | Resolve tenant, user, customer, and channel identity | user/channel/business ids | identity envelope | implemented |
| `customer.load` | Load customer profile and traits | customer id | customer object | implemented |
| `session.load_or_create` | Find or create the active conversation session | business id, customer id, channel | session | implemented |
| `session.history.load` | Load recent messages for the turn | session id, limit/window | message list | implemented |
| `session.summary.load` | Load compressed older context | session id | summary text/metadata | implemented |
| `lifecycle.load` | Load lifecycle and stage state | customer/session id | lifecycle state | partial |
| `intent_stack.load` | Load active flow/plan stack | session id | current intent stack | partial |
| `confirmation.inspect` | Detect pending confirmation or parked operation | session/customer id | confirmation state | implemented |

These are read-only or intake operations. They should not decide what to do;
they only expose what is true right now.

## 2. Transform Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `text.normalize` | Trim, canonicalize whitespace, lower where appropriate | raw text | normalized text | implemented |
| `text.language_hint` | Infer language or mixed-language state | raw text | language hint | planned |
| `generic.classify` | Detect narrow greetings, thanks, farewells, or generic prompts | normalized text, active state | generic decision | implemented |
| `span.split` | Split a message into semantically useful spans | message text | spans | implemented |
| `span.keyword_extract` | Extract exact keywords and anchors from spans | spans | keyword list | partial |
| `value.extract` | Extract ids, amounts, dates, product names, plan names | text | typed values | partial |
| `time.resolve_relative` | Convert "tomorrow", "last week", "in 5 minutes" | text, current time | absolute time range | planned |
| `embedding.create` | Convert text/span/query into vector | text | embedding vector | implemented |
| `embedding.normalize` | Validate dimensions and normalize vector values | vector | normalized vector | partial |
| `args.coerce` | Convert planner/tool args into schema-valid args | raw args, schema | typed args | implemented |
| `query.spec.construct` | Construct a query spec from retrieval intent | intent, filters, vector/text | query request | partial |
| `prompt.compile` | Compile selected facts and instructions into final LLM prompt | prompt sections | prompt text | implemented |

These are pure transforms when possible. If an operation calls an LLM, it should
live in the Reason family, not here.

## 3. Query Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `aelio-db.health` | Check Aelio DB availability | none | health result | implemented |
| `aelio-db.schema.get` | Inspect existing table schema | table name | schema | implemented |
| `aelio-db.table.ensure` | Create or verify a collection/table | table schema | table status | implemented |
| `aelio-db.row.get` | Fetch one row/document by id | table, id | row | implemented |
| `aelio-db.rows.scan` | Scan rows with filters | table, filters, limit | rows | implemented |
| `aelio-db.row.insert` | Insert one atomic record | table, row | inserted row | implemented |
| `aelio-db.row.update` | Update one atomic record | table, id, patch | updated row | implemented |
| `aelio-db.row.delete` | Delete one atomic record | table, id | delete status | implemented |
| `aelio-db.query.vector` | Run vector nearest-neighbor search | vector, filters, k | scored rows | implemented |
| `aelio-db.query.text` | Run lexical/BM25-like search | text, filters, k | scored rows | partial |
| `aelio-db.query.semantic` | Run semantic query plan against vectors/text | semantic query | scored rows | partial |
| `aelio-db.query.graph` | Traverse graph edges or relations | node/edge constraints | graph result | planned |
| `aelio-db.query.hybrid` | Combine vector, lexical, filters, and graph | query request | fused result set | partial |

Aelio DB should become the fast substrate for most read/write atomic operations.
The TypeScript harness should decide when and why to call Aelio DB; Aelio DB should
make the query fast and structured.

## 4. Retrieve Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `immediate_context.write` | Add message to short-term time bucket | message, timestamp | bucket update | implemented |
| `immediate_context.rollup` | Move old context into larger time buckets | bucket state, clock | rolled context | implemented |
| `immediate_context.read` | Pull current 5m/15m/30m/1h/day context | user/session id | context block | implemented |
| `memory.recall` | Retrieve long-term user/business memories | query vector/text | relevant memories | implemented |
| `response_cache.lookup` | Find exact/semantic reusable answer | message/cache key | cache hit/miss | implemented |
| `tool.retrieve` | Retrieve relevant callable tools | intent/query | ranked tools | implemented |
| `policy.retrieve` | Retrieve relevant policies | intent/action/user | policy candidates | partial |
| `flow.retrieve` | Retrieve active or similar flows | current turn | flow candidates | partial |
| `archetype.retrieve` | Retrieve positive/negative/neutral archetype matches | spans, embeddings | archetype evidence | implemented |

Retrieval operations should return evidence, not final decisions. Scoring and
decision operations consume the evidence afterward.

## 5. Evaluate Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `score.cosine` | Score vector similarity | vector A, vector B | similarity number | implemented |
| `score.lexical` | Score lexical match strength | query, document | lexical score | partial |
| `score.rrf` | Fuse ranked result lists with reciprocal rank fusion | ranked lists | fused ranking | planned |
| `score.evidence` | Score source quality, recency, specificity, and match | evidence set | evidence score | planned |
| `score.archetype_valence` | Score positive/negative/neutral stance | archetype matches | valence scores | implemented |
| `score.pathway` | Score semantic pathway/flow/tool fit | pathway candidates | pathway decision | implemented |
| `gate.generic` | Decide whether to bypass expensive harness work | generic classification | bypass/no-bypass | implemented |
| `gate.confirmation` | Decide whether hard confirmation is required | action, policy, args | confirmation decision | implemented |
| `gate.policy` | Decide whether an action is allowed, blocked, or needs escalation | policy candidates, action | policy verdict | partial |
| `gate.confidence` | Decide whether evidence is sufficient or a clarification is needed | scores, thresholds | proceed/clarify | partial |

This family is where the harness earns trust. The LLM can suggest, but these
operations should make final structured decisions where possible.

## 6. Reason Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `llm.classify_intent` | Ask the LLM for bounded intent classification | message, labels | structured intent | partial |
| `llm.plan` | Ask the LLM for a bounded action plan | prompt, tools, policies | plan AST | implemented |
| `llm.replan` | Ask the LLM to repair a failed or blocked plan | error, prior plan | revised plan AST | implemented |
| `llm.summarize` | Compress older conversation context | messages/context | summary | implemented |
| `llm.synthesize_reply` | Convert final decision/evidence into natural language | final prompt, facts | reply text | implemented |
| `llm.discover_aspects` | Discover new stable user/business aspects | messages/evidence | aspect candidates | implemented |
| `llm.critique_action` | Judge whether a proposed action is risky or poorly grounded | plan/action/evidence | critique | planned |

Reason operations are allowed to use model intelligence, but their outputs must
be structured and checked by Evaluate/Govern operations before side effects.

## 7. Act Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `tool.bind` | Select tool and bind candidate args | tool schema, evidence | bound call | implemented |
| `tool.resolve_args` | Fill missing required args from context or ask user | bound call, context | resolved args/need input | implemented |
| `tool.execute` | Invoke an approved local or remote tool | tool call | tool result | implemented |
| `tool.result.normalize` | Convert tool output to harness-safe facts | raw tool result | normalized facts | partial |
| `confirmation.request` | Ask user to approve risky action | action, args, reason | confirmation prompt | implemented |
| `confirmation.apply` | Apply a confirmed pending action | confirmation id, user reply | action decision | implemented |
| `flow.advance` | Move active workflow to the next step | flow state, result | new flow state | partial |
| `state.transition` | Update user/session/business state machine | current state, event | new state | partial |
| `reply.send` | Return final assistant message | reply text, channel | delivery result | implemented |
| `proactive.enqueue` | Schedule or emit proactive followup | user state, strategy | queued/sent/suppressed | partial |

Act operations are the only vocabulary family that should cause business side
effects, external calls, or user-visible messages.

## 8. Control Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `control.sequence` | Run operations in strict order | operation list | ordered results | implemented |
| `control.parallel` | Run independent reads/scores together | operation list | result map | implemented |
| `control.branch` | Choose one path based on a decision | condition, branches | branch result | implemented |
| `control.loop_bounded` | Repeat planning/tool steps with max iterations | loop config | loop result | implemented |
| `control.retry` | Retry transient failure under budget | operation, retry policy | success/failure | partial |
| `control.timeout` | Stop an operation that exceeds budget | operation, timeout | timeout result | partial |
| `control.suspend` | Park a turn waiting for user input/confirmation | state to park | suspension id | implemented |
| `control.resume` | Resume parked plan from new user input | suspension id, message | resumed state | implemented |
| `control.terminate` | End turn with final reply or refusal | final state | terminal result | implemented |
| `control.lock` | Prevent concurrent turns corrupting one session | session id | lock result | implemented |

Control operations compose the harness. They should stay boring and explicit.

## 9. Learn Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `memory.extract` | Extract durable facts/preferences from a turn | messages, reply, tool results | memory candidates | implemented |
| `memory.store` | Persist accepted memory | memory candidate | stored memory | implemented |
| `aspect.discover` | Discover recurring user/business aspect | context, messages | aspect candidate | implemented |
| `aspect.touch` | Update usage, confidence, recency of an aspect | aspect id, evidence | updated aspect | implemented |
| `aspect.promote_retire` | Promote useful aspects or retire stale/noisy ones | aspect stats | aspect lifecycle update | partial |
| `axis.observe` | Attach observation to positive/negative/neutral axis | span, archetype, score | axis observation | partial |
| `graph.edge.connect` | Link user, memory, aspect, tool, flow, or policy nodes | nodes, relation | edge | planned |

Learning must be evidence-bound. The system should not "learn" an inference
unless it can store what text/tool result caused the update.

## 10. Observe And Govern Operations

| Operation | Role | Input | Output | Status |
| --- | --- | --- | --- | --- |
| `trace.write` | Persist one harness decision/action trace | trace event | trace id | implemented |
| `trace.prompt` | Persist prompt construction metadata | prompt sections, mode | prompt trace | implemented |
| `trace.reply` | Persist final reply metadata | reply, model, source | reply trace | implemented |
| `trace.function_call` | Persist tool call and result audit | tool call/result | audit row | implemented |
| `trace.api_call` | Persist external API/model call metadata | api call | audit row | implemented |
| `trace.proactive` | Persist daemon send/suppress/blocked decision | proactive decision | trace row | implemented |
| `admin.turn_reconstruct` | Build human-readable turn overview | turn id | decision overview | implemented |
| `govern.tenant_scope` | Enforce business/user isolation | ids, operation | allow/deny | implemented |
| `govern.prompt_redact` | Redact prompt text before storage if configured | prompt text, policy | redacted prompt | implemented |
| `govern.pii_redact` | Redact sensitive values in traces/context | text/object | redacted object | planned |
| `govern.retention_enforce` | Expire old traces/context/memories by policy | records, policy | retained/deleted | planned |

These operations are why the admin view can explain the harness. Every serious
turn should show the exact operations that made the decision.

## How A Non-Generic Prompt Uses The Vocabulary

User message:

```text
I am still annoyed about the VAT charge. Can you fix the invoice before I renew?
```

Expected atomic operation path:

```text
receive.message
time.now
identity.resolve
session.load_or_create
session.history.load
generic.classify
gate.generic
embedding.create
span.split
immediate_context.write
immediate_context.read
memory.recall
archetype.retrieve
score.archetype_valence
flow.retrieve
tool.retrieve
score.pathway
policy.retrieve
gate.policy
prompt.compile
trace.prompt
llm.plan
tool.bind
tool.resolve_args
gate.confirmation
confirmation.request OR tool.execute
llm.synthesize_reply
reply.send
memory.extract
aspect.discover
trace.reply
admin.turn_reconstruct
```

Important scoring behavior:

- `embedding.create` creates one shared message vector.
- `archetype.retrieve` compares spans against positive, negative, and neutral
  archetype buckets.
- `score.archetype_valence` may produce something like:

```json
{
  "sentiment": { "negative": 0.78, "neutral": 0.21, "positive": 0.08 },
  "urgency": { "negative": 0.63, "neutral": 0.31, "positive": 0.12 },
  "dominantSpan": "I am still annoyed about the VAT charge"
}
```

- `memory.recall` should pull prior VAT/invoice evidence if its vector score,
  lexical match, or hybrid score beats threshold.
- `score.pathway` should prefer invoice correction/account retention paths over
  generic support because the message contains VAT, invoice, fix, and renew.
- `gate.confirmation` should prevent money/account mutations unless the action
  is explicitly allowed or confirmed.

The final prompt should not be "raw memory dump". It should be a compiled,
minimal contract:

```text
You are responding for this business and this customer.

Current user message:
I am still annoyed about the VAT charge. Can you fix the invoice before I renew?

Harness decisions:
- Generic gate: no bypass.
- Pathway: invoice_correction_retention, score 0.84.
- Stance: frustrated/negative, evidence span "annoyed about the VAT charge".
- Relevant memory: prior VAT concern on invoice renewal thread.
- Policy: invoice mutation requires confirmation if amount changes.
- Tool candidates: inspect_invoice, update_invoice, create_retention_note.

Instruction:
Answer directly, acknowledge the frustration, inspect before changing, and ask
for confirmation before any invoice mutation.
```

Then the LLM words the response. The harness, not the LLM, owns the selected
memory, pathway, policy, tool candidates, confirmation rule, and trace.

## Build Direction

The next architectural step is to turn this document into a small runtime
registry:

```ts
const operations = {
  "embedding.create": {
    family: "Transform",
    deterministic: false,
    sideEffects: "external",
    traceKind: "embedding",
    implementation: embedText
  },
  "aelio-db.query.hybrid": {
    family: "Query",
    deterministic: true,
    sideEffects: "read",
    traceKind: "aelio-db_query",
    implementation: aelio-db.query
  }
};
```

Once the registry exists, each turn trace can say:

- operation id
- input hash
- output summary
- score
- duration
- error, if any
- whether it influenced the final prompt

That gives the admin chat exactly what you described: not just the final answer,
but the harness vocabulary and the sequence of independent decisions that built
that answer.
