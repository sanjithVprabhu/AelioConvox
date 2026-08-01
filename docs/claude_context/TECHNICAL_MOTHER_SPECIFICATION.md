# AELIO HARNESS LAYER — TECHNICAL MOTHER SPECIFICATION

**Version:** 1.6
**Status:** NORMATIVE for implementation. Reference-audited: every internal §/appendix/D-n reference resolves; kernel dependencies enumerated under Normative references. Derived from `ideation.md` (capture log) and governed by `AELIO_DSL_MOTHER.md` (the kernel specification, "the mother doc"). Where this document conflicts with a LOCKED mother-doc section, the mother doc wins and the conflict is a defect in this document.
**Audience:** the implementing engineer. Every section is written to be buildable from directly.


**Normative references.** This specification depends on the following LOCKED sections of `AELIO_DSL_MOTHER.md` and does not restate them: mother §1 (thesis and non-goals), mother §4.1 (imprints, two-tier handles, no self-derived conformance), mother §4.2–4.3 (bag semantics, canonicalisation baseline), mother §6 (bag/projection), mother §8.4 (acyclicity, raise-on-exhaustion), mother §10.1–10.4 (registries, effect classes, prompt artifacts, Prism), mother §12 (ledger and replay), mother §13–16 (converters, containment, promotion gate), mother §17.2 (tenant scoping, cold-path emission ban), mother §18 (embedding lifecycle and selection discipline), mother §29 (Planner), mother App G (ledger records), and mother App J (prompt template format). Mother Amendment #4 ratifies Prism as the canonical flexible-read AST. The convergence rules and implementation sequence are in `AELIO_UNIFIED_IMPLEMENTATION_MASTER_PLAN.md`.

**Contents.**
§0 Thesis · §1 Vocabulary · §2 Silos (2.1 mint · 2.2 kernel · 2.3 bank/Prism · 2.4 harness · 2.5 reactor · 2.6 page · 2.7 glu · 2.8 steward · 2.9 sandbox) · §3 Artifact model · §4 Build spec (4.1 Build result) · §5 The builder and the twelve-step spine · §6 Kernel operations · §7 Prompt operations · §8 Reactor · §9 Bank rules · §10 Glu · §11 Steward · §12 Demand loop · §13 Bootstrap + falsification · §14 Reason codes · §15 Security · §16 Trust levels · §17 Deliberate absences · §18 Implementation order · §19 Decision log · §20 Gap resolutions · §21 Hardening pass · §22 Residual risks · App A Imprints · App B Seed prompts · App C Collections · App D Code→operation matrix · App E Conformance · App F Worked trace · App G Determinism rules · App H Constants

**Conventions.**
- `MUST` / `MUST NOT` / `MAY` are normative.
- `k.` prefixes a deterministic kernel operation. `p.` prefixes a prompt (model-calling) operation. There is no third kind of step.
- `id@N` is a pinned artifact reference. An unpinned reference is illegal everywhere.
- **[D-n]** marks a ratified decision, indexed in §19 (Decision Log).

---

## 0. Thesis

> **Everything that is alive is a harness. Nothing that guarantees is.** [D-1]

The Aelio kernel guarantees: it validates, executes, ledgers, gates, terminates. It never adapts. The harness layer adapts: it decides, decomposes, selects, composes, authors. It never guarantees. The harness layer is a **client** of the kernel, never a peer, and every artifact it produces passes through the kernel's gate before anything depends on it.

The layer exists to make the system **self-extending under verification**: capabilities are built from a one-sentence specification, by a machine, out of parts that already exist — and nothing a model produces is trusted until a machine has checked it.

### 0.1 The stack

| Layer | Component | Computing analogue |
|---|---|---|
| 0 | Aelio kernel: interpreter, Planner, gate, ledger, Prism executor | CPU + OS |
| 1 | Sol node vocabulary (`Call`, `Branch`, `Map`, `Loop`, …) + kernel-op primitives | instruction set |
| 2 | Flows / harness artifacts (pinned, executable) | programs |
| 3 | Harnesses that build layer-2 artifacts | compilers |
| 4 | The root harness (builds harnesses, including builders) | compiler-compiler |
| ⟂ | Prompt artifacts (mint-forged) | typed oracle interface, cuts across 2–4 |

Because a harness contains a model, it cannot be deterministic; therefore it cannot be trusted the way a compiler is trusted (reproducibility). **The substitute for determinism is: (a) exhaustive pinning — the artifact names every dependency exactly — and (b) the gate — declared examples must pass in the sandbox before promotion.** [D-2]

### 0.2 Guarantees of the harness layer

Inherited from the kernel: G1 termination, G2 replay, G4 no self-derived conformance, G5 least privilege (mother doc numbering). Added by this layer:

- **H1 — Verified construction.** No built artifact is executable by a real tenant until it has passed `k.validate` (static) and `k.gate` (behavioural, in sandbox).
- **H2 — Bounded building.** Every build terminates: hard structural caps (`max_depth`, `max_children`) plus one threaded, atomically-decremented consumable budget. Exhaustion raises; no partial tree is ever returned as success.
- **H3 — Mechanical insufficiency.** "The registry lacks X" is only ever concluded by a deterministic check (Planner unknown-target, registry miss), never by a model's self-report.
- **H4 — Checked oracles.** Every `p.` step's output flows into a `k.` step that verifies it before it flows anywhere else. (The p→k invariant, §7.1.)
- **H5 — No trust laundering.** An artifact's trust level ≤ min(its own gate verdict, trust of everything it pins). Reuse, conversion, and promotion all preserve this.
- **H6 — Off-path authoring.** Builds run at authoring time or from the demand queue — never inline in a live conversation. Runtime executes pinned artifacts only. [D-3]

---

## 1. Vocabulary

| Term | Definition |
|---|---|
| **Silo** | One of the seven subsystems: mint, kernel, bank, harness, reactor, page, glu (+ steward, sandbox as infrastructure). |
| **Primitive** | An atomic step: one prompt → one model call → closed-schema output → fixed exits (`p.` primitive), or one registered deterministic operation (`k.` primitive / kernel op). Has no inner structure. |
| **Kind vs instance** | Primitive *kinds* are the finite, human-authored floor. *Instances* are unbounded and factory-produced. Recursion bottoms out on kinds. [D-4] |
| **Step kinds (CLOSED SET)** | Exactly four, matching `reaction.kind` (A.8): `k` (kernel op), `p` (prompt call), `recall` (Prism read), `sub_harness` (pinned nested call). Extending this set is a change to THIS document's §1, not something any builder or configuration can do. This four-row table IS the floor of D-4. |
| **Harness** | A tree of steps. A step is either a nested harness reference (pinned) or a primitive. |
| **Reaction** | One step execution at runtime: one input, one output. The universal unit: **1 reaction = 1 ledger entry = 1 budget decrement = 1 replay checkpoint.** [D-5] |
| **Seam** | A `(producer_step, consumer_step)` junction inside a harness. Owned end-to-end by glu (§10). |
| **Imprint** | A declared, versioned data shape (mother §4.1). All interfaces are imprints. |
| **Build spec** | The input contract of the builder (§4). |
| **Trust level** | `debris < held < canary < promoted`. Only `canary` and `promoted` are executable; only `promoted` artifacts' descriptions may be rendered into prompts (§16). |
| **Axiom** | A human-authored, versioned, vendor-origin artifact that no machine may rewrite: `root` (mint) and `root_harness`. [D-6] |

---

## 2. The silos

### 2.1 aelio-mint — the prompt factory

Every LLM use in the system is a prompt artifact with: **objective** (authoring metadata), **slots** (typed, sensitivity-tagged inputs), **output imprint** (a contract, not an example), **exemplars** (worked cases that MUST validate against the imprints), pinned model + params, version. Storage shape is the mother doc's App J template; `prompt_hash` covers the resolved composition.

**Unification rule.** *Thinking* (decision output that advances a workflow) and *synthesis* (the reply itself) are the same shape: synthesis is a prompt whose output imprint is `{reply: string, ...}`. There is exactly one prompt mechanism in the system. [D-7]

**`root` — the mint axiom.** The only prompt that authors prompts. Human-written, versioned; `root` MUST NOT author or modify `root`. Forge call:

```text
forge(objective, slots[], output_imprint, exemplars[]) -> prompt_artifact@v, trust=canary
```

**Sufficiency discipline [D-8]:**
1. `root` MUST emit an explicit sufficiency judgement: can the declared slots answer the objective? On failure it refuses with `slots_insufficient` — it does not forge a confabulation machine.
2. Every forged prompt's output imprint MUST include a legal `undeterminable` variant, so the built prompt can decline rather than invent.

**Gate predicate for prompts:** the exemplars. A forged prompt promotes when it reproduces its declared exemplars in the sandbox. [D-9]

### 2.2 aelio-kernel — deterministic operations

The registry of deterministic operations: control flow, arithmetic/analytics, file and structured I/O, page operations, handle dereference. Two classes:

- **Primitives** — human-authored, closed set (add, count, divide, read_file, page.put, …). The floor.
- **Derived ops** — compositions of existing ops (average = sum/count). Open set.

**The kernel root is not a separate machine.** [D-10] It is the root harness (§5) invoked with configuration `{policy.allowed_effects: ["compute"], scope.registries: ["kernel.*"]}`. A derived-op request either composes from existing ops or fails with a bounded error code. The kernel root composes sequences; it MUST NOT emit code. New *primitives* are human work, demanded by the insufficiency log.

### 2.3 aelio-bank — storage and retrieval

Aelio DB-backed, four modalities per collection: relational records, full-text, vector, graph. Every artifact in the system (prompts, harnesses, kernel ops, glus) is a bank record carrying `{id, version, interface, description, description_vec, tags, effects, trust_level, pins, hash}`.

**Bank root** = one command, two interpretations: create a **collection** (DDL: declared fields, types, which fields are text-indexed, which are vector-indexed and under which pinned embedding model, filter mode per §2.3.2) or create an **entry** (DML, imprint-validated). Entry/relation/edit operations are `write`-effect kernel ops: policy gate + ledger mandatory.

#### 2.3.1 Prism — the query layer

Surface syntax for humans, compiled at push time to a closed canonical AST; **only the AST is stored, hashed, executed. No query text exists at runtime; an LLM may only ever fill AST fields.**

```json
{"from":"<literal-collection-id>",
 "match":[{"kind":"key|text|vector|graph|fusion", "...":"closed kind fields"}],
 "where":[{"col":"<literal-field>","op":"eq|ne|lt|le|gt|ge","value":"<scalar>"}],
 "select":["<literal-field>",...], "limit":int>0, "into"?:"<literal-page-path>"}
```

The v1 match set is exactly `key | text | vector(vector|embed) | graph | fusion(rrf)`, with at most one text, vector, graph, and fusion clause. More than one ranked modality requires exactly one explicit RRF fusion clause; raw scores across modalities are never compared. Graph clauses require bounded seeds, `max_depth`, and `max_nodes`. `select` and `limit` are mandatory; table dumps, `order`, cursors, joins, subqueries, expressions, and computed field names do not exist in v1. `into` is optional only at the pure database boundary and mandatory when Prism is called from the kernel. `tenant_id` is not addressable — the runtime applies it structurally.

`where` is a flat AND of closed scalar predicates. Multi-step retrieval is sequential recall. The legacy single-operation `QueryAst` and `recall(...)` helper are authoring sugar compiling to this envelope; only Prism is stored and executed.

#### 2.3.2 Semantics rulings

- **Filter × vector [D-11, refined by Amendment #4]:** the database planner chooses exact pre-filter or recall-targeted post-filter from measured selectivity/index capability. Results are always bounded by `limit`; fewer rows than `limit` is an ordinary observable underfill, never padded or fabricated.
- **Write atomicity [D-12]:** the relational commit **is** the commit. Text/vector/graph indexes follow asynchronously; every record carries `indexes_committed[]`, exposed in recall results. Read-your-write callers check the flag.
- Vector-indexed collections inherit the pinned-embedding-model lifecycle (mother §18): model bump ⇒ re-embed + demote dependents.

### 2.4 harness + root harness

The orchestration silo. Fully specified by §§4–8 of this document; §2 lists it here only for silo completeness.

### 2.5 aelio-reactor — the runtime

Executes pinned harness artifacts. Walks the tree; each step execution is a **reaction**. Per reaction: resolve pinned target → glu seam pipeline on inputs (§10) → execute (`k.` op, `p.` prompt call, bank recall, or sub-harness push) → glu on output → ledger entry → budget decrement. Crash recovery replays the ledger to the last completed reaction and resumes. `p.` reactions are never re-called on replay — their recorded results are injected (mother §12).

### 2.6 aelio-page — working memory

The per-execution typed JSON bag. Values: literals, arrays, objects, bytes, **handles** (pointers to large data; two-tier per mother §4.1 — dereference is an explicit kernel op). `page.put/get/remove/keys` are kernel ops. Every step reads only its declared projection of the page (G5); nothing sees the whole page.

### 2.7 aelio-glu — the seam subsystem

Glu owns every seam end to end, as one deterministic pipeline [D-13]:

```text
producer output
  → validate against producer's declared output imprint      (parse-or-reject)
  → convert iff imprints differ, via a registered, pinned converter
  → validate against consumer's declared input imprint       (parse-or-reject)
  → consumer input
```

Converters are drawn from the mother doc's closed, non-computational rule set; models never author transformations. A glu instance is `{glu_id, from: op_id@v, to: op_id@v, converter@v}`. When no registered converter exists, glu refuses with a bounded error code (`no_converter`, `validation_failed_producer`, `validation_failed_consumer`, `handle_type_mismatch`). Glu error codes are a closed vocabulary owned by glu.

Glu instances are built by the glu harness — which is the root harness configured over converter-rule scope [D-10 corollary] — and gated like everything else.

### 2.8 aelio-steward — the lifecycle watcher

Split out of glu [D-14]: watching is a registry concern, not a seam concern. The steward is a daemon over the registry + insufficiency log:

- **Departures.** Tool/op/harness goes inactive or is demoted ⇒ every artifact pinning it is demoted (cascade, mother §10.3.4 pattern). The steward fires the cascade.
- **Arrivals.** A new promoted artifact's interface/description is matched against logged insufficiencies; on match the steward **proposes** a rebuild into the demand queue.
- **The steward MUST NOT promote.** [D-15] Its proposals enter the normal build path and land at `canary`. A watcher that promotes is a second, ungated authoring path.

### 2.9 sandbox — the shared verification tenant

One default tenant, existing for everybody, solely for tests and gate runs. [D-16]

- **Data rule [D-17]:** only declared example data and synthetic fixtures may enter. No real-tenant record may be copied in — the sandbox would otherwise be a cross-tenant exfiltration channel. Bank/page namespaces are wiped between runs.
- All gate example-runs execute here, including `write`-effect harnesses (writes land in sandbox namespaces).
- **Promotion out of sandbox re-runs the gate in the destination tenant's pin context** (embedding models are per-tenant pins). [D-18]
- Failed-build partial trees are retained here only, storage class `debris`: structurally unreadable by `k.resolve`, `k.search`, and the gate; TTL-swept. [D-19]

---

## 3. The artifact model

Every buildable thing — prompt, harness, derived kernel op, glu — is an **artifact**:

```json
{"id":"harness.write_md_report", "version":3, "hash":"<blake3 canonical>",
 "class":"harness|prompt|kernel_op|glu",
 "interface":{"inputs":[{name, imprint@N, required, sensitivity}], "output":"imprint@N"},
 "description":"...", "tags":[...], "effects":["read","write","compute","external"],
 "trust":"canary|promoted|held",
 "pins":{"steps":[], "prompts":[], "sub_harnesses":[], "converters":[], "models":[], "embeddings":[]},
 "examples":[{inputs, output}],       // inside the artifact — edits are version bumps [D-20]
 "provenance":{built_by:"root_harness@v", prompts_invoked:[{id, prompt_hash}], requester, tenant, at},
 "body": <class-specific: step tree + seams | App-J template | op sequence | converter binding>}
```

Rules:
- **Versioning.** Same name + identical interface ⇒ version bump (legal). Same name + different interface ⇒ collision (rejected). Interface equality = multiset `{(input name, imprint, required)}` + output imprint; description is never compared.
- **Pinning.** Every reference inside `body` and `pins` is `id@N`. An unpinned reference fails `k.admit`/`k.validate`. Auto-resolving to latest is forbidden.
- **Trust propagation (H5).** `trust(artifact) ≤ min(gate verdict, min over pins)`. Recomputed at `k.assemble`; enforced at candidate time by `k.search`'s trust filter.
- **Demotion cascade.** A pinned dependency demoting demotes all dependents (steward-fired).

---

## 4. The build spec — input contract of the builder

```json
{"name":"harness.tool_selector",
 "description":"given a task and a tool registry, select the tools required",   // ONLY free-text field
 "inputs":[{"name":"task","imprint":"task_spec@1","required":true,"sensitivity":"internal"}],
 "output":"tool_selection@1",
 "budget":{"max_depth":4,"max_children":6,"max_llm_calls":40,"max_wall_ms":120000},
 "scope":{"registries":["steps.core","harness.flow.*"],"tenant":"<tid>"},
 "policy":{"allowed_effects":["read"],"denied_effects":["write","external"]},
 "examples":[{"inputs":{...},"output":{...}}]}
```

- `description` is the only free text; `p.decompose` reads it and nothing else. One field = one origin for quality problems.
- **Budget semantics [D-21].** `max_depth`/`max_children` are **structural caps, copied** per node (`max_depth` decrements with depth). `max_llm_calls`/`max_wall_ms` are **consumable, threaded**: one atomic counter shared by the entire recursive build, decremented by every `p.` reaction at any depth; decrement-or-fail, no read-then-write. Exhaustion raises `budget_exhausted` wherever it lands.
- `policy.allowed_effects` MUST ⊆ the requester's own grants (no self-escalation) and MUST NOT intersect `denied_effects`.
- `examples` are simultaneously: admission-checked types (§6 check 7), the reuse verifier (§6 `k.resolve` stage 4), and the gate predicate. **Examples MUST assert stable properties only** — never bytes, timestamps, or ordering of unordered fields.

### 4.1 The build result — output contract

One envelope, three outcomes:

```json
{"status":"built|reused|failed", "spec_hash":"<blake3>",
 "cost":{"llm_calls":n,"tokens":n,"wall_ms":n,"depth_reached":n,"retries":n},
 "ledger":["<ref>",...], ...payload}
```

- `reused`: pinned reference to the found artifact (distinct from `built` for cost accounting and reuse metrics).
- `built`: the artifact (§3) + gate verdict.
- `failed`: `{stage, spec_path[], reason_code, detail, missing[], insufficiency_ids[]}` — `reason_code` from the closed vocabulary of §14; free text only in `detail`.

---

## 5. The builder — one machine, many configurations [D-10]

There is exactly **one** builder: the root harness. Every other "root" that composes existing operations is the same machine under a restricted configuration:

| Invocation | Configuration |
|---|---|
| Root harness (general) | full scope, effects per spec |
| Kernel root | `allowed_effects=["compute"]`, `scope=["kernel.*"]` |
| Glu harness | scope = converter rule registry |
| Flow builder | scope = tool + step registries, output class = flow |

Only mint's `root` is a different machine (it authors prompt *text*, not compositions).

**The root harness is itself an artifact** whose spec is expressible in the build-spec format (§4). Its own spec is written by hand as the first entry in the registry (the stage-0 consistency check): if the builder's interface cannot be stated in its own input language, the fixpoint claim fails structurally.

### 5.1 The twelve-step spine

```text
root_harness :: build_spec@1 -> build_result@1

 1  k.admit             validate + resolve        -> ok(version, spec_hash) | reject
 2  k.resolve           stage 0-4 funnel          -> exact -> RETURN reused | candidates | none
 3  k.search            2 Prism queries, RRF      -> ranked candidates
 4  p.select            scored pick               -> selection | abstain -> 7
 5  p.compose           tree + seams              -> draft
 6  k.validate          Planner                   -> accept -> 10
                                                  -> unknown_target: widen 3 once, else 7
                                                  -> type_mismatch:  retry 5 once, else 7
                                                  -> effect|cycle|unbounded -> 12
 7  p.decompose         split                     -> children | atomic -> 12 | cannot -> 12
 8  k.admit_children    caps, decrease, cycle,
                        interface closure         -> ok | reject -> 12
 9  k.recurse           BUILD each child          -> built[] | failures -> 12
10  k.assemble          tree, pins, effects,
                        trust cap, hash           -> artifact
11  k.validate + k.gate Planner + sandbox examples-> RETURN built (canary|promoted)
12  k.fail              diagnostic + log          -> RETURN failed
```

Control-flow properties: the algorithm moves forward except two single-retry backedges (widen search; recompose with diagnostic). Reuse precedes construction; construction precedes decomposition; decomposition happens only after a *machine* has ruled composition impossible.

### 5.2 Termination (H2) [D-22]

Termination rests **entirely on hard caps**: bounded depth (decrementing `max_depth`), bounded branching (`max_children`), bounded oracle spend (threaded atomic budget), single-retry backedges, and mother-doc bounds inside every executed op. Declared complexity (`p.decompose` emits an integer per child; `k.admit_children` enforces child < parent) is a **quality** check: a model gaming it can waste budget but cannot break termination, and waste is bounded by the threaded counter.

### 5.3 `atomic` and the two axioms [D-23]

When `p.decompose` returns `atomic` (indivisible, and nothing in the registry builds it), v1 **fails and logs** — it does not forge. `p.forge_step` (the harness builder calling mint's `root` to author a missing primitive) is deferred until the insufficiency log demonstrates which gaps are model-fillable. The two axioms stay parallel and independent in v1.

---

## 6. Kernel operation specifications

Three families: **gatekeepers** (`k.admit`, `k.admit_children`, `k.validate`, `k.gate` — the only four builder operations that may reject candidate construction), **retrievers** (`k.resolve`, `k.search`), **constructors** (`k.recurse`, `k.assemble`, `k.fail`). All nine are deterministic and unit-testable without a model. Runtime prompt parsing, Glu, policy, budget, and effect boundaries retain their separately closed mother-doc failure channels.

### 6.1 `k.admit` — validate AND resolve

Runs at every node (children re-enter here). Returns a **normalised spec** — version assigned, imprints resolved to handles, scope expanded, `spec_hash` computed — not a boolean. Checks, cheapest first:

1. **Parse** against `build_spec@1` → `malformed_spec`
2. **Canonicalise** → `spec_hash = blake3(canonical)`
3. **Name**: absent → v1; present + interface identical → version bump vN+1; present + interface differs → `name_collision`
4. **Imprints**: each matches `^[a-z][a-z0-9_.]*@v[0-9]+$` (else `unpinned_imprint`) and resolves (else `unknown_imprint`). Never auto-resolve to latest.
5. **Budget**: caps > 0; depth ≤ max_depth; shared counter not exhausted
6. **Policy**: `allowed ∩ denied = ∅`; `allowed ⊆ grants(principal)` (else `escalation`)
7. **Scope**: registry patterns expand; tenant may read all
8. **Examples** type-check against resolved input/output imprints → `example_type_mismatch`

`k.admit` never judges whether `description` is achievable — no kernel step makes semantic judgements. Checks 3–4 are the per-child verification of `p.decompose` output (complementary to `k.admit_children`: individual vs collective).

### 6.2 `k.resolve` — the reuse funnel

Stage 0: `spec_hash` lookup — identical spec previously built and promoted → return it.
Stage 1: interface compatibility (set ops): output identical (covariant); candidate's *required* inputs ⊆ spec's supplied inputs (contravariant); candidate optionals ignored.
Stage 2: policy: candidate effect union ⊆ `allowed_effects`.
Stage 3: purpose ranking — one Prism vector query over `description` across survivors. Types constrain; they do not identify.
Stage 4: **example verification** — run the spec's examples against the top candidate in sandbox. Pass → `exact` (pin, return `reused`). Fail → demote to `candidates`.

Rules: reuse without declared examples is refused (returns `candidates`, never silent similarity-only reuse) [D-24]. Trust never launders: reusing a `canary` artifact caps the parent at `canary` (H5). Highest qualifying version wins and is recorded in pins.

### 6.3 `k.search`

Deterministic keyword extraction (stopword strip — never a model), then one Prism call:
`fuse(rrf, [vector(description_vec, embed=spec.description, model=pinned), text(tags, keywords)])`, `where effects ⊆ allowed_effects AND trust_level ≥ build trust`, `limit 25 + 25·widen_level`.
Widening is a declared ladder (level 1: drop text sub-query, raise limit, widen to parent registries), so the single permitted retry is reproducible. Empty results are not a rejection.

### 6.4 `k.validate` — the Planner, classified

1. every target `id@v` resolves → `unknown_target(ids)` — **this is H3's insufficiency signal**
2. per seam: imprints identical → ok; registered converter exists → ok + pin it; neither → `type_mismatch(seam)`
3. effect union ⊆ `allowed_effects` → `effect_violation`
4. call graph acyclic → `cycle_detected`
5. every loop capped, every query limited → `unbounded`

Routing: 1 → widen-once-then-decompose; 2 → recompose-once; **3–5 → fail immediately, no retry** — guarantees are not negotiable with a model.

### 6.5 `k.admit_children`

Four checks, each fatal, all RAISE (never a partial tree):
1. breadth ≤ `max_children`
2. strict decrease: every `child.complexity < parent_complexity` (quality check per §5.2)
3. oscillation: no child matches any ancestor on the current path (match defined in G.5)
4. **interface closure** (reachability on the bipartite producer/consumer graph): every child input is supplied by the parent's inputs or a sibling's output; the children's outputs jointly cover the parent's declared output. This is what makes a split *wireable* before any child is paid for.

### 6.6 `k.recurse`

Builds admitted children in **topological order of the seam graph** (not emission order), passing: copied structural caps (depth−1), the **shared atomic budget handle**, and the ancestor path (for the oscillation guard). Child-failure policy [D-25]: build-all within remaining budget (maximises the gap list for the insufficiency log); fail-fast when budget is the binding constraint. If children ever build concurrently, budget decrements MUST be atomic decrement-or-fail.

### 6.7 `k.assemble`

Purely mechanical: construct the final tree from built/reused children + the seam map; collect `pins` transitively; compute the effect union; **cap trust at min over pins (H5)**; canonicalise; hash. Cannot reject.

### 6.8 `k.gate`

Runs the artifact's declared examples in the sandbox tenant, under the destination tenant's pin context when promoting out of sandbox [D-18]. Predicate: all examples pass under G.3 subset-match semantics. Verdicts: `promoted` | `canary` (default for v1 artifacts) | `held`. This is the mother doc's generic gate with `examples` as the agreement predicate — uniform across harnesses, prompts (exemplars), derived ops, and glus.

### 6.9 `k.fail`

Terminal constructor of the failure envelope + one insufficiency-log entry per gap. Runs on the unhappy path of every operation, therefore every operation MUST supply `(reason_code ∈ §14, spec_path)`.

---

## 7. Prompt operation specifications

### 7.1 The p→k invariant (H4)

> **The prompt reports; the kernel decides.** Every `p.` output flows into a `k.` check before it flows anywhere else.

`p.select → k.` thresholds · `p.compose → k.validate` · `p.decompose → k.admit_children` (+ per-child `k.admit`) · `p.forge_step → k.gate`. Any design in which a `p.` output flows onward unchecked is a defect, auditable by reading the step names. Counting `p.` steps in an artifact yields its model-call cost.

### 7.2 `p.select`

**In:** spec + candidate table — each row `id@v` with **full interface** (the model is choosing things that must wire; prose alone cannot support that). **Out (closed schema):** `{selected:[{id, score, role}], runners_up:[...], unmet:[...]}`.

Kernel post-processing: (a) any selected id not in the offered table → registry lookup; real-but-outside-top-k → admit; nonexistent → record `invented_target`, drop (this catches hallucinated tools one operation before the Planner would); (b) apply τ (top-1 threshold), δ (margin over top-2), entropy ceiling → `selection` or `abstain`. The model is never asked whether it is confident. `unmet` is captured even on success and feeds the insufficiency log.

### 7.3 `p.compose`

**In:** spec + selected steps with full interfaces. **Out:** `{tree:[{nid, calls:id@v, args:{literal page paths}}], seams:[{from, to, from_imprint, to_imprint}]}`.

Kernel post-checks before the Planner: nids unique; every `calls` resolves; every args value a *literal* path (no computed/templated paths); seam endpoints exist; graph acyclic. `p.compose` declares that seams exist and names both imprints; **it never authors a conversion** — glu resolves seams (§10).

### 7.4 `p.decompose`

**In:** `{spec, depth_remaining, unmet[] from p.select, diagnostic from k.validate}` — **NOT the candidate list** (decomposition is reached because selection over those candidates failed; showing them again biases the split toward what exists). **Out:** `{verdict: children|atomic|cannot, parent_complexity:int, children:[{name, description, inputs[declared imprints], output:imprint@N, complexity:int}]}`.

Children MUST carry full declared interfaces — otherwise closure is uncheckable and wireability is unknown until every child is built and paid for.

### 7.5 `p.forge_step` (deferred, v2) [D-23]

Calls mint's `root` to author a missing prompt primitive when a leaf is `atomic`. Manufactures a missing part; never arranges existing ones. Output enters at `canary` through the standard gate.

---

## 8. The reactor — execution semantics

Given a pinned harness artifact and an input page:

```text
for each step in tree order:
    project page -> step's declared args only (G5)
    glu inbound: validate (producer imprint) [first step: validate input against harness interface]
    execute:  k. op | p. prompt (pinned model, prompt_hash ledgered) | bank recall (Prism AST) | sub-harness (push frame)
    glu outbound: validate -> convert (pinned converter) -> validate (consumer imprint)
    write result into page at declared path
    ledger one logical reaction checkpoint, grouped by reaction_id
      (an effect reaction may append intent + dispatch + result entries)
    decrement consumable budget (atomic)
```

Replay: pure function over the ledger; `p.`, external, channel/Sense, nondeterministic, and `recall` observations are injected from the record after their argument hashes verify. Pure `k.` reactions and conversions re-execute and MUST match (hard-refuse on divergence). Sub-harness calls are reactions in the caller and open their own reaction sequence within the same ledger stream.

---

## 9. Bank interaction rules

- **`from` is always a literal collection id in every runtime Prism AST** — never computed, never model-chosen. Consequence: each artifact's full read-set over the bank is derivable statically at push time (hydration order computable per flow, preserving mother §10.4's derivability property).
- Artifact selection at **build time** is `k.search`/`k.resolve` (Prism), with results **pinned** into the artifact. At **runtime** there is no similarity-based selection of prompts, harnesses, or ops — runtime lookups are by pinned id only. Conditional behaviour is `Branch` over pinned alternatives, never vector search.
- `recall(collection, query, modality)` survives as surface sugar compiling to a full Prism AST with `select`/`limit` from collection defaults.
- The query-builder harness (need → Prism AST) is an authoring-time tool, separately gated; most call sites write their recall directly.

## 10. Glu — seam resolution and runtime enforcement

**Build time** (`k.validate` step 2): for each declared seam, imprints identical → identity; registered converter → pin `glu_id`; neither → `type_mismatch` (which may drive one recompose, then decomposition). **Run time** (§8): the validate→convert→validate pipeline executes per reaction boundary using only pinned converters. Glu failures at runtime are bounded error codes and halt the reaction with a ledgered fault — they are never "best-effort passed through."

## 11. Steward — lifecycle propagation

Registry event stream → { departure: fire demotion cascade over the pin graph; arrival: match interface+description against open insufficiency entries → enqueue rebuild proposal }. Proposals carry the originating insufficiency ids; results close those entries on promotion. The steward has no authoring or promotion authority (§2.8).

## 12. The demand loop — how the system grows [D-3]

```text
runtime request -> no pinned harness matches
  -> honest decline to the user ("can't do that yet")
  -> insufficiency entry {needed, why, requester, spec sketch}
  -> demand queue -> root harness build (off-path)
  -> sandbox gate -> canary -> promoted
  -> capability exists for subsequent requests
```

Runtime never executes an artifact born in the same breath. The system grows from real demand, always through the gate. The insufficiency log is thereby the system's demand signal: a ranked, evidence-backed list of what to author next — for the builder (rebuild proposals), and for humans (new primitives, which no machine authors in v1).

## 13. Bootstrap sequence

1. Hand-author the axioms: `root` (mint) and `root_harness` — versioned, vendor-origin, never machine-modified.
2. Hand-write the root harness's own build spec as registry entry #1 (fixpoint consistency check, §5).
3. Hand-forge the seed prompts through `root`: `prompt.decompose@1`, `prompt.select@1`, `prompt.compose@1` (each with slots, output imprint incl. `undeterminable`, exemplars).
4. Register kernel primitives (closed human set) and base collections (bank root DDL), sandbox tenant, base imprints (`build_spec@1`, `build_result@1`, `selection_result@1`, `compose_result@1`, `decompose_result@1`).
5. **Falsification test:** feed the root harness the flow-builder spec. Outcomes: builds and its output builds → fixpoint confirmed; needs hand-editing → refuted (two builders); valid but unusably generic → machine right, `prompt.decompose` weak — a mint problem, not an architecture problem. Distinguishing those two failure modes is the point of the test.

## 14. Reason codes (closed vocabulary)

```
malformed_spec  name_collision  unpinned_imprint  unknown_imprint  bad_budget
depth_exceeded  breadth_exceeded  budget_exhausted  escalation  policy_incoherent
scope_unreadable  example_type_mismatch  no_candidates  select_abstained
invented_target  unknown_target  type_mismatch  effect_violation  cycle_detected
unbounded  no_strict_decrease  oscillation_detected  interface_not_closed
child_failed  composition_invalid  examples_failed  slots_insufficient
atomic_but_unbuildable  indecomposable  no_converter  validation_failed_producer
validation_failed_consumer  handle_type_mismatch  name_reserved  inefficient_artifact
model_output_invalid
```

Free text lives only in `detail`. The closed vocabulary is what makes the insufficiency log aggregable into a demand signal.

## 15. Security and containment

1. **Injection via registry descriptions:** only *promoted* artifacts' descriptions may be rendered into any prompt [D-26]. Layered containment: gate upstream of every description; closed output schemas mean the worst achievable outcome of a poisoned description is a wrong selection; wrong selections are precisely what `k.validate` and gate examples catch.
2. **No query text at runtime** (Prism AST only); filter grammar non-computational; literal paths only in args.
3. **Sandbox data rule** [D-17] (§2.9). 4. **No self-escalation** (§6.1 check 6). 5. **Trust anti-laundering** (H5). 6. **Axioms are machine-immutable** [D-6]. 7. `tenant_id` structurally applied, never addressable.

## 16. Trust levels

`debris` (sandbox-only wreckage, invisible to retrieval and gate) → `held` (gate-failed, inert) → `canary` (gate-passed v1 default; executable under canary routing; caps parents at canary) → `promoted` (full trust; description renderable into prompts; reusable without capping).

## 17. What is deliberately absent from v1

`p.forge_step` (axiom cooperation) — until the log shows model-fillable gaps. Live in-conversation building — forbidden by H6, replaced by the demand loop. Machine-authored kernel primitives — human-only. Imprint refinement/subtyping in `k.resolve` stage 1 — identical-output only. Cursor semantics for Prism paging — stateless-token design pending.

## 18. Implementation order

1. Imprints + artifact model + bank collections (everything depends on declared shapes)
2. Prism (AST executor; surface compiler can lag)
3. Glu pipeline (validate–convert–validate) — required by reactor and validate
4. `k.admit`, `k.resolve`, `k.search`, `k.validate`, `k.admit_children`, `k.assemble` — the four genuinely new deterministic ops plus wiring (Planner/gate/ledger exist per mother doc)
5. Reactor (reaction loop + ledger + replay)
6. Sandbox tenant + gate wiring
7. Seed prompts through `root`; register axioms
8. Twelve-step spine end to end; run the §13 step-5 falsification test
9. Steward + demand queue

## 19. Decision log

| # | Decision |
|---|---|
| D-1 | Alive/guaranteeing split is the governing thesis |
| D-2 | Pins + gate substitute for compiler determinism |
| D-3 | Authoring-time + demand-queued builds; runtime executes pinned artifacts only |
| D-4 | Kind/instance: kinds are the finite floor; instances unbounded |
| D-5 | Reaction = universal unit (ledger/budget/replay) |
| D-6 | Two axioms, human-authored, machine-immutable: `root`, `root_harness` |
| D-7 | Synthesis and thinking unified as one prompt shape |
| D-8 | Mint sufficiency: root refuses insufficient slots; `undeterminable` mandatory in forged outputs |
| D-9 | Gate predicate = examples/exemplars, uniform across artifact classes |
| D-10 | One builder; kernel root / glu harness / flow builder are configurations |
| D-11 | Vector filters: planner-selected exact pre-filter or recall-targeted post-filter; bounded result length exposes underfill without fabrication |
| D-12 | Relational commit is the commit; async indexes with `indexes_committed[]` |
| D-13 | Glu = validate–convert–validate; owns seams end to end |
| D-14 | Steward split from glu |
| D-15 | Steward proposes, never promotes |
| D-16 | One shared sandbox tenant for all verification |
| D-17 | Sandbox admits only example/synthetic data |
| D-18 | Cross-tenant promotion re-gates under destination pins |
| D-19 | Partial trees = `debris`, sandbox-only, TTL |
| D-20 | Examples live inside the versioned artifact |
| D-21 | Structural caps copied; consumable budget threaded + atomic |
| D-22 | Termination by hard caps; declared complexity is a quality check |
| D-23 | `atomic` fails and logs in v1; forge deferred |
| D-24 | No automatic reuse without examples |
| D-25 | Child failure: build-all within budget, else fail-fast |
| D-26 | Only promoted descriptions render into prompts |
| D-27 | Selection thresholds run on measured similarity; model scores advisory |
| D-28 | Harness evidence is subordinate to mother App K: steward proposes, gate executes; ≥20 distinct canary inputs, ≥98% success, zero Guard violations; reviewed approval occurs before first canary consumption |
| D-29 | Example coverage admission-enforced: min counts, negative path, input coverage |
| D-30 | Assemble hash-dedupe + steward efficiency audit (`inefficient_artifact`) |
| D-31 | Name admission = atomic reservation with lease; in-flight dedupe by spec_hash |
| D-32 | Resolve stage-4 verdicts content-address cached |
| D-33 | Runtime p-failure: one re-sample, then `model_output_invalid` fault; `undeterminable` flows as a typed value |
| D-34 | Sensitivity enforced statically: secret never reaches a prompt; hash-only ledgering |
| D-35 | Pin-only departures → re-gate proposals, not rebuilds; embedding migration recomputes vectors first |
| D-36 | Examples carry synthetic fixtures, gate-loaded into the sandbox namespace |
| D-37 | Empty candidates skip select; empty selection = abstain |
| D-38 | Insufficiency entries dedupe on (needed, reason_code); count = queue priority |
| D-39 | Gate runs budgeted; decision prompts pinned at temperature 0 (seeded) under prompt_hash |
| D-40 | Global pin graph is a DAG by construction; no cross-artifact cycle detector |
| D-41 | 2026-08-01 convergence: mother Amendment #4 Prism is canonical; exact-code-point Sol hashing, replay-injected reads, mother App K lifecycle, and mother paths override the superseded technical variants |


## 20. Gap resolutions (v1.0 → v1.1)

Six gaps identified post-draft; each resolved **using machinery the spec already has** — no new subsystems.

### 20.1 Selection confidence runs on measurements, not model scores [D-27]

Model-emitted scores are not calibrated; a model emits 0.83 identically when right and when guessing. Resolution — re-ground §7.2's thresholds in quantities the kernel measures:

- `k.search` already produces real measurements per candidate: vector similarity and RRF rank. These are carried forward into the candidate table.
- `p.select` still emits its scores, but they are **advisory ordering only** — never thresholded.
- Kernel confidence check: a selection passes iff **every selected candidate's measured similarity ≥ τ** and the selected set's minimum measured similarity clears the runner-up field by δ. Entropy ceiling applies to the *measured* distribution.
- The model MAY select a low-similarity candidate it believes necessary; if any selected item fails measured-τ, the kernel routes to `abstain` → decompose. The model cannot talk its way past a measurement.
- Ultimate arbiter unchanged: `k.validate` + `k.gate`. τ/δ are now honest pre-filters on real numbers rather than fake statistics on self-reports.

### 20.2 Canary → promoted has a mechanical trigger [D-28, aligned to mother App K]

Promotion is a **second gate run with a ledger-evidence predicate**, steward-proposed, gate-executed (D-15 preserved: the steward still never promotes).

- While `canary`, the artifact executes under canary routing; every reaction is ledgered as normal.
- Steward watches the ledger. When an artifact accumulates **at least 20 distinct canary input hashes, at least 98% downstream success, and zero attributed Guard violations** inside the observation window, it enqueues a promotion proposal. Repeated identical inputs never inflate evidence.
- `k.gate` re-runs: examples must still pass **and** the ledger evidence must be attached. Verdict `promoted`.
- PII/fabricating artifacts and artifacts with reachable `write` or `external` effects are reviewed; human approval gates `shadow → canary`, before first consumption. Secret artifacts are locked and never automatically proposed.
- Attributed failure above 2% or any Guard violation requests immediate demotion to `shadow`; the lifecycle repository, never the watcher itself, commits the transition.

### 20.3 Example quality is admission-enforced [D-29]

`k.admit` check 8 is extended from type-checking to **coverage-checking**:

1. **Minimum count** per class: harness ≥ 3 examples; prompt ≥ 3 exemplars.
2. **Negative-path coverage**: at least one example MUST exercise the refusal path — for prompts, an exemplar whose correct output is the `undeterminable` variant; for harnesses, an example whose expected output is a declared error/decline shape. A gate that has never seen the artifact refuse has not tested it.
3. **Input coverage**: every required input exercised in every example; every optional input exercised in ≥ 1 example.

All three are deterministic counts over declared data — they enforce example *breadth*, and remain honest about not enforcing semantic quality, which stays with the falsification loop and 20.4.

### 20.4 Bad-but-wireable artifacts: dedupe at assemble, audit by steward [D-30]

Two mechanisms, one preventive and one retroactive:

- **`k.assemble` hash-dedupe** (preventive, free): identical subtrees — same canonical hash — are merged into one pinned node before hashing the artifact. Redundant decomposition collapses mechanically.
- **Steward efficiency audit** (retroactive): `k.assemble` already records structural metrics (`depth_reached`, node count, reuse ratio) and the ledger records per-reaction cost. The steward periodically flags promoted artifacts whose cost or size exceeds a declared per-class percentile and files an insufficiency entry of a new class — `inefficient_artifact` — into the demand queue, proposing a rebuild. The rebuild competes through the normal path; the old version demotes only if the new one promotes and dominates on the same examples + ledger evidence.

Bad-but-wireable is thus bounded at build time (caps), collapsed where detectable (dedupe), and *hunted* in production by the same demand loop that hunts missing capability.

### 20.5 Name admission is a reservation, not a read [D-31]

`k.admit` check 3 becomes an **atomic compare-and-reserve** against the registry:

- Reserve `(tenant, name, interface_hash)` → lease bound to `spec_hash`, with TTL covering `budget.max_wall_ms` plus gate time. Registration at gate consumes the lease; failure or TTL expiry releases it.
- Concurrent build, **same `spec_hash`** → do not build twice: subscribe to the in-flight build and return its result (extends `k.resolve` stage 0 to in-flight builds).
- Concurrent build, **different `spec_hash`, same name+interface** → `name_reserved` (new reason code): retry after the lease resolves, at which point it is a clean version bump.
- Different interface → `name_collision` as before.

### 20.6 Stage-4 verification is cached [D-32]

`k.resolve` stage 4 (run the spec's examples against the top candidate) writes its verdict to a kernel-written bank collection keyed `(candidate id@v hash, examples hash)` → `{pass|fail, ledger_ref}`. Keys are content-addressed and therefore immutable: a candidate version bump or an example edit (itself a version bump, D-20) produces a new key naturally — no invalidation logic exists because none is needed. Stage 4 becomes: cache hit → verdict; miss → one sandbox execution → write-through.

## 21. Hardening pass (v1.3 → v1.4)

Eight further gaps found on adversarial re-read; resolved within existing machinery.

### 21.1 Runtime `p.` failure semantics [D-33]

Build-time retries were specified; runtime was silent. Normative rules for a `p.` reaction during real execution:

- **Parse failure** (output does not validate against the prompt's output imprint): one re-sample permitted, same prompt, same pinned params. Second failure → reaction fault `model_output_invalid`, execution halts, fault ledgered, the harness returns its declared error/decline shape. Both samples are ledgered; on replay the *successful* sample (or the fault) is injected — replay never re-samples.
- **`undeterminable` is not a fault.** It is a legal value of the output imprint and *flows*: any consumer of a prompt whose imprint carries the variant sees it in the type and MUST handle it (typically a `Branch`). `k.validate` seam-checks this like any other shape — a consumer that cannot accept the variant is a `type_mismatch` at build time, not a surprise at runtime.

### 21.2 Sensitivity enforcement [D-34]

`SlotDecl.sensitivity` was declared but never enforced. Rule: **`secret` values MUST NOT be rendered into any `p.` step, ever** — projection into a prompt slot fails at plan time (`policy_violation`). `secret` values may flow only through `k.` ops and glu, and are ledgered as `result_hash` only (never `result`). `internal` renders into prompts but is masked in any artifact `description`, `detail`, or insufficiency text. `public` is unrestricted. Enforced statically at `k.validate` (a seam from a secret-carrying path into a `p.` slot is rejected) — a guarantee, not a runtime check.

### 21.3 Model-pin migration [D-35]

A pinned model's deprecation is a departure event, and the naive cascade demotes every artifact in the system. That is correct but wasteful — the artifacts are not wrong, their oracle changed. Migration path: the steward emits **re-gate proposals**, not rebuild proposals, for pin-only departures (model or embedding version): re-run `k.gate` on the same artifact body under the successor pin. Pass → new version, same body, updated pin, trust preserved. Fail → then demote and enqueue a true rebuild. Embedding-pin migration additionally triggers `description_vec` recompute (bank-side) before any search runs under the new space (mother §18 — scores never compared across spaces).

### 21.4 Example fixtures [D-36]

Gate runs execute in an empty sandbox namespace — but a harness whose steps include bank recalls cannot pass any example against an empty bank. `ExampleCase` gains an optional field:

```json
"fixtures": [{"collection":"string","records":["object (validating against the collection imprint)"]}]
```

`k.gate` loads fixtures into the run's namespace before execution (G.11). Fixtures are inside the artifact ⇒ example edits remain version bumps (D-20); fixture records are subject to the sandbox data rule (D-17) — synthetic only, admission-checked like everything else.

### 21.5 Empty-candidate shortcut; empty selection = abstain [D-37]

If `k.search` returns zero candidates, steps 4–6 are skipped — `p.select` over an empty table is a wasted model call — and control passes directly to `p.decompose` with `unmet = ["no candidates in scope"]`. Separately: a `p.select` output with `selected: []` is treated by the kernel as `abstain` regardless of scores. `no_candidates` as a reason code appears only in failure envelopes when the subsequent decomposition path also fails, recording *why* composition was never attempted.

### 21.6 Insufficiency dedup [D-38]

Unbounded identical gap entries would flood the demand queue. The steward dedupes on `hash(needed_normalised, reason_code)`: an existing open entry increments `count` and appends the requester; priority in the demand queue is ordered by `count` desc. The log thereby becomes a *ranked* demand signal mechanically, not by later analysis.

### 21.7 Gate execution budget and sampling [D-39]

Gate runs are executions and get a budget: `gate_budget` per artifact class (H.1) — wall-clock and reaction caps; exhaustion = `examples_failed` with the budget fault in detail. Sampling defaults for decision prompts (`decompose`/`select`/`compose` and all forged classifier-shaped prompts): temperature 0, fixed seed where the provider supports it — pinned in the prompt artifact params, therefore covered by `prompt_hash`. Synthesis-class prompts may pin non-zero temperature; their gate examples must then assert only stable properties (G.3 makes this natural).

### 21.8 Cross-artifact acyclicity is free — stated, not engineered [D-40]

`k.validate` checks acyclicity within a draft. Across artifacts no check is needed: **pins are versioned and may only reference artifacts that exist at build time**, so the global pin graph is a DAG by construction — A@1 ↔ B@1 mutual reference would require each to precede the other. Recorded as a property so nobody builds a redundant global cycle detector.


## 22. Residual risk register — what remains open by nature, not by omission

A specification that hides residual risk is less safe than one that names it. These four cannot be closed by any further writing; each has a declared containment and a declared measurement.

| # | Residual risk | Why no spec can close it | Containment | Measured by |
|---|---|---|---|---|
| R1 | **Retrieval relevance has no oracle.** A recall can return valid, non-empty, irrelevant results and "succeed." | Meaning is not provable (mother §5.1). | Runtime recalls are pinned queries authored at build time and exercised by gate examples with fixtures (D-36); never model-improvised. | Gate example failures on recall-bearing harnesses; production fault-free-but-wrong reports feeding the insufficiency log. |
| R2 | **Seed prompt quality** — `p.decompose` inventing coherent intermediate imprints is the load-bearing model capability. | Empirical, not specifiable. | Falsification test outcome 4 explicitly separates prompt weakness from architecture failure; iteration touches only App B bodies, never structure. | Rounds of seed iteration needed to pass §13 step 5; `interface_not_closed` frequency in the log. |
| R3 | **Threshold calibration** (τ, δ, H_MAX, OSC_SIM) — similarity is not suitability. | Values are corpus- and embedder-dependent. | Miscalibration routes to `abstain`/decompose (safe direction: waste, never wrongness); all values versioned in `kernel.config@1`, changes ledgered. | `select_abstained` vs `invented_target` vs downstream `examples_failed` ratios per config version. |
| R4 | **Sandbox throughput** — one shared verification tenant serializes gate runs at scale. | Ops, not architecture. | G.11 namespaces are per-run and independent; parallelism is permitted by construction. | Gate queue latency; runs per hour. |

Everything else raised anywhere in the design history — 27 numbered open questions and 9 numbered conflicts in `ideation.md` — is either resolved by a D-numbered decision in this document, or explicitly deferred in §17 with its trigger condition. None is silently dropped; the audit mapping is recorded in `ideation.md` §25.



---

# APPENDICES — NORMATIVE CONTRACTS

Everything an implementer would otherwise have to guess. Appendix A is the ground truth for every shape named in the body; where body prose and Appendix A differ, A wins.

## Appendix A — Core imprints

Schemas in compact JSON-Schema-like notation. `!` = required. All artifact ids match `^[a-z][a-z0-9_.]*$`; all pins match `^[a-z][a-z0-9_.]*@[1-9][0-9]*$` (`PIN` below).

### A.1 `build_spec@1`
```json
{ "name!":        "string (id)",
  "description!": "string, <= 500 chars",            // the ONLY free text
  "inputs!":      [{"name!":"string","imprint!":"PIN","required!":"bool",
                    "sensitivity":"public|internal|pii|secret (default internal)"}],
  "output!":      "PIN",
  "budget!":      {"max_depth!":"int>0","max_children!":"int>0",
                   "max_llm_calls!":"int>0","max_wall_ms!":"int>0"},
  "scope!":       {"registries!":["string glob"],"tenant!":"string"},
  "policy!":      {"allowed_effects!":["read|write|compute|external"],
                   "denied_effects!":["..."]},
  "examples!":    [ExampleCase],                     // >=3, coverage per D-29
  "trust_target": "canary|promoted (default canary)" }

ExampleCase = {"inputs!":"object (validates against inputs[])",
               "output!":"object (validates against output imprint)",
               "kind":"positive|negative (default positive)",   // >=1 negative required
               "fixtures":[{"collection":"string","records":["object"]}]}   // D-36; sandbox-loaded, synthetic only
```

### A.2 `build_result@1`
```json
{ "status!":"built|reused|failed", "spec_hash!":"hex64",
  "cost!":{"llm_calls":"int","tokens":"int","wall_ms":"int","depth_reached":"int","retries":"int"},
  "ledger!":["string ref"],
  "artifact":  Artifact,          // status=built
  "reused":    {"ref!":"PIN"},    // status=reused
  "failure":   Failure }          // status=failed

Failure = {"stage!":"admit|resolve|select|compose|validate|decompose|admit_children|child|assemble|gate",
           "spec_path!":["string"], "reason_code!":"enum §14", "detail":"string",
           "missing":[{"needed":"string","why":"string","at":["string"]}],
           "insufficiency_ids":["string"]}
```

### A.3 `artifact@1` (bank record, all classes)
```json
{ "id!":"string", "version!":"int>=1", "hash!":"hex64 (blake3 canonical body+interface+examples)",
  "class!":"harness|prompt|kernel_op|glu",
  "interface!":{"inputs!":[SlotDecl],"output!":"PIN"},
  "description!":"string", "description_vec":"float[] (bank-computed, pinned embedder)",
  "tags":["string"], "effects!":["read|write|compute|external"],
  "trust!":"debris|held|canary|promoted",
  "pins!":{"steps":["PIN"],"prompts":["PIN"],"sub_harnesses":["PIN"],
           "converters":["PIN"],"models":["PIN"],"embeddings":["PIN"]},
  "examples!":[ExampleCase],
  "provenance!":{"built_by!":"PIN","prompts_invoked":[{"id":"PIN","prompt_hash":"hex64"}],
                 "requester!":"string","tenant!":"string","at!":"iso8601",
                 "decomposition_tree":"object|null"},
  "metrics":{"node_count":"int","depth":"int","reuse_ratio":"float"},   // D-30
  "body!":"HarnessBody | PromptBody | OpSequenceBody | GluBody" }

SlotDecl = {"name!":"string","imprint!":"PIN","required!":"bool","sensitivity":"..."}
HarnessBody = {"tree!":[{"nid!":"string","calls!":"PIN","args!":{"<slot>":"literal page path"}}],
               "seams!":[{"from!":"nid|$input","to!":"nid|$output",
                          "from_imprint!":"PIN","to_imprint!":"PIN","glu":"PIN|identity"}]}
PromptBody  = App-J template (mother doc) + {"objective":"string"}
OpSequenceBody = {"ops!":[{"op!":"PIN","args!":{...}}]}          // kernel-root output
GluBody     = {"from_op!":"PIN","to_op!":"PIN","converter!":"PIN"}
```

### A.4 `selection_result@1`  (p.select output)
```json
{ "selected!":[{"id!":"string (id or PIN)","score!":"float 0..1 ADVISORY","role!":"string"}],
  "runners_up":[{"id":"...","score":"float"}],
  "unmet":["string"],
  "undeterminable":"bool (default false)" }
```
Kernel thresholds run on **measured** similarity carried per candidate from `k.search` (D-27), never on `score`.

### A.5 `decompose_result@1`
```json
{ "verdict!":"children|atomic|cannot",
  "parent_complexity!":"int>0",
  "children":[{"name!":"string","description!":"string","complexity!":"int>0",
               "inputs!":[SlotDecl],"output!":"PIN"}],
  "reason":"string (atomic|cannot)",
  "undeterminable":"bool" }
```

### A.6 `compose_result@1`
```json
{ "tree!": HarnessBody.tree, "seams!": HarnessBody.seams,   // glu field omitted — kernel fills
  "undeterminable":"bool" }
```

### A.7 `insufficiency@1` (bank collection `insufficiency_log`)
```json
{ "id!":"string", "class!":"missing_capability|inefficient_artifact",
  "needed!":"string", "why!":"string", "spec_path":["string"],
  "reason_code!":"enum §14", "requester":"string","tenant":"string","at!":"iso8601",
  "spec_sketch":"partial build_spec|null",
  "status!":"open|queued|building|closed", "closed_by":"PIN|null" }
```

### A.8 `reaction@1` (ledger record)
```json
{ "reaction_id!":"string (monotonic per execution)", "execution_id!":"string",
  "step!":"PIN", "kind!":"k|p|recall|sub_harness",
  "args_hash!":"hex64", "result":"object|null", "result_hash!":"hex64",
  "prompt_hash":"hex64 (kind=p)", "glu":{"in":"PIN|identity","out":"PIN|identity"},
  "fault":"glu|validate|budget|null", "cost":{"tokens":"int","wall_ms":"int"},
  "at!":"iso8601" }
```
Replay: pure `k` reactions MAY re-execute and MUST hash-match. `recall`, `p`, external, Sense/channel, and other nondeterministic observations are INJECTED from the record after argument-hash verification; they are never re-called.

### A.9 `lease@1` (collection `name_leases`, D-31)
```json
{ "key!":"(tenant,name,interface_hash)","spec_hash!":"hex64","build_id!":"string",
  "expires_at!":"iso8601" }
```

### A.10 `verification_cache@1` (D-32)
```json
{ "key!":"(candidate_hash, examples_hash)", "verdict!":"pass|fail", "ledger_ref!":"string" }
```

### A.11a `collection_config@1` (bank root DDL output)
```json
{ "collection!":"string (id)", "record_imprint!":"PIN",
  "fields!":[{"name!":"string","type!":"string","text_indexed":"bool","vector_indexed":"bool"}],
  "embedder":"PIN (required iff any vector_indexed)",
  "filter_mode!":"pre|post",                      // D-11
  "filterable_fields!":["string"],                // whitelist for Prism where-clauses
  "kernel_written":"bool (default false)",        // App C: ledger/cache/leases
  "defaults":{"select":["string"],"limit":"int"} }
```
Plan-time validation of every Prism `where`/`select` runs against this record; a field outside `filterable_fields` or `fields` is rejected at push time.

### A.11 `promotion_proposal@1` (D-28)
```json
{ "artifact!":"PIN", "evidence!":{"executions":"int","distinct_inputs":"int","faults":"int",
  "window":"iso8601/iso8601","ledger_refs":["string"]},
  "requires_human!":"bool (pii OR fabricating rules OR downstream reach intersects {write,external})",
  "status!":"pending|approved|promoted|rejected" }
```

## Appendix B — Seed prompt templates (hand-forged through `root`, v1)

Slots and output imprints are normative; body text is a starting point the falsification loop will iterate. All three declare `undeterminable` outputs (D-8). Sensitivity: all slots `internal`.

### B.1 `prompt.decompose@1`
- slots: `spec_name`, `spec_description`, `spec_inputs (rendered SlotDecl[])`, `spec_output`, `depth_remaining:int`, `unmet:string[]`, `diagnostic:string`
- output: `decompose_result@1`
- body (layered): *You are decomposing a build objective into strictly smaller sub-objectives. You will NOT be shown candidate components — reason from the objective and the reported gaps only. Every child must declare full typed inputs and one typed output using ONLY imprint ids that appear in the parent's interface or that you introduce as clearly named new imprints for intermediate values. Children must jointly produce the parent output; every child input must come from the parent inputs or a sibling output. Emit integer complexity per child, each strictly less than the parent's. If the objective is a single indivisible capability, verdict=atomic. If it cannot be meaningfully decomposed, verdict=cannot. If you cannot determine a valid split, set undeterminable=true rather than guessing.*
- exemplars: ≥3 incl. one `atomic` and one `undeterminable`.

### B.2 `prompt.select@1`
- slots: `spec_description`, `spec_interface`, `candidates (rendered rows: id@v, full interface, description, measured_similarity)`
- output: `selection_result@1`
- body: *Choose the minimal set of components whose interfaces can be wired to satisfy the objective. Judge by interfaces first, descriptions second. Scores you emit are advisory. List every capability the candidate set cannot cover in `unmet`, even if you select. If no coherent set exists, select nothing and fill `unmet`; if you cannot judge, set undeterminable=true.*
- exemplars: ≥3 incl. one empty-selection and one `undeterminable`.

### B.3 `prompt.compose@1`
- slots: `spec_interface`, `selected (rendered with full interfaces)`
- output: `compose_result@1`
- body: *Arrange the selected components into a dependency tree. For every junction, declare a seam naming the producer's output imprint and the consumer's input imprint exactly as declared — do NOT transform data, rename fields, or invent conversion logic; mismatched seams are expected and are handled elsewhere. Args must be literal page paths. Use $input/$output for the harness boundary. If a valid arrangement does not exist, set undeterminable=true.*
- exemplars: ≥3 incl. one `undeterminable`.

## Appendix C — Bank collections (DDL summary)

| Collection | Indexed | Notes |
|---|---|---|
| `artifacts` | relational + text(tags) + vector(description_vec) + graph(pins edges) | filter mode: pre-filter; embedder pinned per tenant |
| `insufficiency_log` | relational + text + vector(needed) | steward match surface |
| `demand_queue` | relational | FIFO + priority = open-insufficiency count |
| `name_leases` | relational | TTL-swept |
| `verification_cache` | relational | content-addressed, immutable |
| `promotion_proposals` | relational | human-approval flag surfaced |
| `ledger` | relational (append-only) | reaction stream; kernel-written only |

Kernel-written collections (`ledger`, `verification_cache`, `name_leases`) accept writes from kernel principals only — no harness may write them.

## Appendix D — Reason code → producing operation matrix

| Code | Produced by |
|---|---|
| malformed_spec, name_collision, name_reserved, unpinned_imprint, unknown_imprint, bad_budget, escalation, policy_incoherent, scope_unreadable, example_type_mismatch | k.admit |
| depth_exceeded | k.admit (per-node) |
| no_candidates | k.search (empty after widen, reported via k.fail) |
| select_abstained, invented_target | p.select kernel post-check |
| unknown_target, type_mismatch, effect_violation, cycle_detected, unbounded | k.validate |
| breadth_exceeded, no_strict_decrease, oscillation_detected, interface_not_closed | k.admit_children |
| atomic_but_unbuildable, indecomposable | p.decompose verdicts via k.fail |
| child_failed, budget_exhausted | k.recurse |
| composition_invalid | k.validate (assembly pass) |
| examples_failed | k.gate |
| slots_insufficient | mint root |
| no_converter, validation_failed_producer, validation_failed_consumer, handle_type_mismatch | glu (build: k.validate step 2; run: reactor) |
| inefficient_artifact | steward audit |
| model_output_invalid | reactor (runtime p-reaction, after one re-sample) |

## Appendix E — Conformance checklist

An implementation conforms iff all hold:

1. Every `p.` output is parsed against its declared imprint; parse failure = the step fails (G4 — no self-derived conformance).
2. The p→k invariant holds for every step sequence (H4) — verifiable by static inspection of harness bodies.
3. No query text, computed field name, computed arg path, or model-authored converter exists anywhere at runtime.
4. The four deterministic builder gatekeepers reject candidate construction with closed reason codes. Runtime prompt parsing, Glu validation, policy, budget, and effect boundaries may also fail with their separately closed mother-doc errors; no free-text error controls execution.
5. Consumable budget is a single atomic counter per build; over-spend is impossible by construction.
6. Termination: with all model calls replaced by adversarial stubs (max children, max complexity claims, always-plausible outputs), every build still terminates within caps.
7. Replay of any ledgered execution is bit-identical on `k` reactions and injected on `p`/`external`.
8. Trust: constructing any promoted artifact that pins a canary artifact is impossible.
9. Sandbox: no real-tenant record reachable from any sandbox execution.
10. Axioms: no code path writes `root` or `root_harness` artifacts.
11. The root harness's own spec exists as registry entry #1 and validates against `build_spec@1`.
12. Falsification test (§13, step 5) is scripted and runnable on demand.

## Appendix F — Worked trace

The canonical end-to-end example (build of `harness.write_md_report`: search-miss on an invented target, decomposition into render+write, reuse of `safe_file_write@4`, six model calls, canary) is maintained in `ideation.md` §19 and MUST be kept consistent with this spec; under D-27 and §7.2 post-checks, the invented target is now caught at selection post-check (operation 4) rather than at the Planner (operation 6), reducing the trace to five model calls.

## Appendix G — Determinism and computation rules

Every rule an implementer would otherwise choose. These are normative; changing any of them changes hashes and breaks replay.

### G.1 Canonical form (input to every hash)
UTF-8 JSON; object keys sorted bytewise by code point at every depth; no insignificant whitespace; integers are bare decimal; floats use shortest round-trip decimal and always retain a decimal point; `-0.0` normalises to `0.0`; strings preserve exact code points with **no Unicode normalization**; arrays keep declared order; `null` fields present in the schema are serialized and absent optionals omitted. These rules are exactly mother §4.3; Technical artifacts do not define a second canonical form.

### G.2 Hashes
- `hash(x)` = BLAKE3 of canonical form, lowercase hex, 64 chars.
- `spec_hash` = hash(normalised build spec AFTER k.admit resolution, EXCLUDING `budget` and `trust_target`) — two requests differing only in budget are the same build.
- `artifact.hash` = hash({interface, examples, body, pins}) — provenance and metrics excluded (descriptive, not identity).
- `interface_hash` = hash({inputs as sorted multiset of (name, imprint, required), output}).
- `prompt_hash` = per mother App J: hash of the fully resolved template composition (layers + slots + output imprint + model pin + params).
- `args_hash` (reaction) = hash(projected args object after glu inbound).
- `result_hash` = hash(canonical result). Large results: `result` field null, payload stored as handle, `result_hash` over the dereferenced bytes.

### G.3 Example comparison semantics (gate + resolve stage 4)
An example passes iff `actual` **subset-matches** `example.output`: every field present in the example must be present in actual and deep-equal after canonicalisation; fields absent from the example are unasserted. Arrays compare element-wise in order unless the example wraps the array as `{"$set":[...]}`, which compares as multisets. This is the mechanical form of "examples assert stable properties only" — omit what is unstable. Negative examples (`kind:"negative"`) pass iff actual matches the declared refusal/`undeterminable` shape.

### G.4 Page path authoring sugar (args values)
`$input`, `$output`, and `$<nid>` are builder-only symbolic roots. Before Mother planning, the builder resolves them to ordinary literal paths governed exclusively by mother §6.1 and Annex F2, including quoted-bracket keys and integer indexes. No symbolic root, wildcard, function, or computed path survives into a stored executable program. `k.` post-checks reject unresolved sugar.

### G.5 Oscillation check (`k.admit_children` check 3)
A child *matches an ancestor* iff `interface_hash(child) == interface_hash(ancestor)` **or** cosine(description_vec(child), description_vec(ancestor)) ≥ `OSC_SIM` (H.1). Description vectors are computed with the tenant's pinned embedder at admission time and cached on the path frame.

### G.6 Keyword extraction (`k.search`)
Lowercase; split on non-alphanumerics; drop tokens of length < 3; drop tokens in the pinned stopword list artifact `kernel.stopwords.en@1` (a versioned artifact, NOT a library default — swapping it is a version bump); keep first 12 remaining tokens in order; dedupe preserving first occurrence.

### G.7 Fusion and thresholds
- RRF: score(c) = Σ_q 1/(RRF_K + rank_q(c)), RRF_K per H.1; ties broken by (higher measured vector similarity, then lexicographic id).
- Measured similarity (D-27) = the raw cosine from the vector sub-query (not the fused score). Candidates reached only via the text sub-query carry similarity computed on demand against the spec embedding before thresholding.
- Selection passes iff min-similarity over selected ≥ τ AND (min selected similarity − max unselected candidate similarity) ≥ δ is NOT required — δ applies as: (top selected similarity − top runner-up similarity) ≥ δ; AND normalised Shannon entropy of the softmax over selected-candidate similarities ≤ H_MAX.
- All three constants per H.1; overridable per collection config, never per call.

### G.8 Deterministic ordering
- `k.recurse` builds children in topological order of the seam graph; ties broken by lexicographic child name.
- Assemble emits tree nodes in reaction order (topological, same tie-break); seams sorted by (from, to).
- Registry candidate lists are always sorted before rendering into `p.select` (fused score desc, then id) so prompt rendering is deterministic.

### G.9 Budget accounting
`max_llm_calls`: decrement 1 per `p.` reaction attempt (including retries and post-check-rejected outputs). `max_wall_ms`: checked before each reaction dispatch against elapsed monotonic clock; a reaction may overrun (never killed mid-flight) but no new reaction dispatches after exhaustion. Token ceilings, if configured, decrement on the model's reported usage. All decrements go through the single atomic counter object; decrement-or-fail.

### G.10 Async-index consistency during builds
`k.search`/`k.resolve` read only records with the relevant index in `indexes_committed[]`. A just-promoted artifact may therefore be invisible to vector search briefly; this is correct behaviour, not a bug — builds see a consistent, possibly slightly stale registry. Stage-0/interface stages (relational) see it immediately.

### G.11 Sandbox reset
Each gate run executes in a fresh namespace `sandbox/<build_id>/<run_n>`: empty page, bank namespace containing ONLY fixtures declared by the examples. Namespace destroyed after verdict; `debris` artifacts are the sole survivors, tagged with the namespace id.

## Appendix H — Constants and defaults

All values live in the config collection `kernel.config@1` and are versioned; these are the shipped defaults. Changing a value is a config version bump and is ledgered.

| Constant | Default | Used by |
|---|---|---|
| τ (min measured similarity) | 0.60 | §7.2 / G.7 |
| δ (top-1 − top-2 margin) | 0.08 | §7.2 / G.7 |
| H_MAX (normalised entropy ceiling) | 0.85 | §7.2 / G.7 |
| OSC_SIM (oscillation similarity) | 0.92 | G.5 |
| RRF_K | 60 | G.7 |
| search limit / widen step | 25 / +25 | §6.3 |
| widen ladder | levels 0,1 only; level 1 miss is terminal | §6.3 |
| resolve stage-3 exact threshold | 0.85 | §6.2 |
| canary distinct inputs / success | 20 / 98%, zero Guard violations | D-28 / mother App K |
| canary observation window | 14 days | D-28 |
| lease TTL | budget.max_wall_ms + 10 min | D-31 |
| debris TTL | 7 days | D-19 |
| min examples (harness / prompt) | 3 / 3, ≥1 negative | D-29 |
| description max length | 500 chars | A.1 |
| p.select candidate table max rows | 25 | §7.2 |
| default budget (top-level build) | depth 4, children 6, llm 40, wall 120 s | §4 |
| stopword list | kernel.stopwords.en@1 | G.6 |
| gate_budget (per run) | 200 reactions / 60 s wall | D-39 |
| decision-prompt sampling | temperature 0, seeded | D-39 |
| runtime p re-sample limit | 1 | D-33 |
| insufficiency dedup key | hash(needed_normalised, reason_code) | D-38 |



— end of specification —
