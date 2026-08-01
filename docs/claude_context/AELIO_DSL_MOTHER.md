# Aelio DSL — Mother Document (Normative Specification & Implementation Reference)

**Document:** `AELIO_DSL_MOTHER.md`
**Status:** v1.0 — ALL 30 SECTIONS LOCKED (2026-07-26). Implementation may cite any section. Amendments via Decision Log only.
**Rule of this document:** Nothing here is prose for its own sake. Every section is either `LOCKED` (implementable as written, survived its attack list), `DRAFT` (decided but unverified), or `OPEN` (undecided). Code may only cite `LOCKED` sections. The synopsis (`aelio-dsl-summary-synopsis.md`) is narrative; this document is law.

**Section status legend:**
- 🔒 `LOCKED` — normative; survived attack list; implementation may proceed
- 📝 `DRAFT` — design exists; attack list unresolved
- ❓ `OPEN` — no decision yet

**Decision log:** Appendix D records every status change with date + rationale.

---

# PART I — POSITIONING & GUARANTEES

## §1. Problem statement & non-goals — 🔒 LOCKED (2026-07-26)

Aelio is an **agent operating system**: a closed kernel of ops, flows composed from them, Sol Contracts as the wire language, Aelio DB as durable store, and an LLM used to *author and repair* logic under a promotion gate — never to be the unsupervised kernel per turn. Warm path = deterministic retrieval + execution; cold path = LLM proposal → gate → reuse.

**Scope (A1.1 resolved):** Aelio serves **conversational and agentic flows over declared tools, states, and pathways, deployed multi-tenant B2B**. It refuses: general-purpose computation (no recursion, no unbounded iteration — by design, §8.4), open-ended autonomous planning (all pathways and decision points are declared), and arbitrary code execution (the instruction set is closed; the only executable artifacts are gated, registered, versioned).

**Non-goals (normative):** LLM-as-kernel (breaks replay, policy, cost, verifiability) · general-purpose language · prompt/personality framework (lives above the OS).

## §2. The escape-hatch argument — why a DSL and not generated code — 🔒 LOCKED (2026-07-26)

The adversarial question every reviewer will ask: *why wouldn't the LLM author Python once, store it, and replay that?* Answer: seven guarantees generated code cannot give, **each traced to a locked mechanism**:

| # | Guarantee | Mechanism (locked) |
|---|---|---|
| G1 | Guaranteed termination | Termination theorem: acyclic call graph + capped loops/lists + deadlined externals + bounded queries (§8.4, §10.4) |
| G2 | Bit-identical replay of any turn | Ledger + replay-as-pure-function + hard-refuse divergence (§12) |
| G3 | Policy gates that structurally cannot be bypassed | Executor-enforced effect invariants; plan-time reject (§10.2) |
| G4 | Structural verifiability of every LLM output | Closed instruction schema; declared-imprint-only validation of Model output (§4.1.2c, §10.3) |
| G5 | Least-privilege at every boundary | Args-only projection for Calls, Map children, prompt slots (§4.2.4, §6.3, §10.3.1) |
| G6 | Safe multi-tenant learned logic | Generic promotion gate + reach-based review + absolute tenant isolation (§16, §17) |
| G7 | Migration of stored programs across versions | 5-point doctrine; refuse-unknown; canary-all on kernel bump (§25) |

**Honest cost column (A2.2):** the DSL loses the Python ecosystem, general expressiveness, and familiar debugging. Compensation: expressiveness loss is the *point* (what can't be expressed can't go wrong — G1/G3/G5 are purchased with it); ecosystem loss is bounded by the registry (any external capability is one registered target away); debugging loss is repaid with interest by §26 (bit-identical replay + op-level bag diffs beat printf in any language). **Falsifiability (A2.3):** the claims above ship with numbers — the mutation harness (§16.6) and pre-registered kill criteria (§21) mean a skeptical reader doesn't have to trust the prose; they can run the harness.

## §3. Threat model — 🔒 LOCKED (2026-07-26)

| Adversary / failure source | Primary defenses (locked) |
|---|---|
| Buggy LLM output (malformed instructions, hallucinated ops) | Closed schema parse-or-reject (§4, §5); plan-time checks (§6.2, §8) |
| **Semantically wrong promotions** (the `active≡loggedin` class — #1 by expected damage) | Gate phases + reach-based reviewed tier + fabrication escalation + advisory judge (§16, §14); strict edge-only scope (§4.1.6) |
| Malicious/compromised tenant (poisoning, cold-path injection) | Absolute tenant isolation (§17); containment theorem (§15); args-only prompt perimeter (§10.3); token-based wake auth (§23); per-tenant quotas (§17.4) |
| Drift (deployer schema changes; distribution shift) | Read-set applicability (§4.1.5 — stale mappings can't fire on drifted data); imprint/template/kernel version-bump demotions (§4.1.8, §10.3.4, §25.5); fallback-rate drift detector (§18) |
| Runtime faults (crash windows, partial persistence) | Universal intent/result accounting (§12.4); at-most-once + fail-loud unknown (§8.4); CAS (§24); ledger-as-WAL (§12.5) |
| Kernel evolution under stored programs | §25 doctrine; refuse-unknown-versions; pinned continuations |

Full guarantee × threat × section matrix: Appendix B. *(A3.1, A3.2 resolved — trust boundary: no learned artifact crosses tenants, §17.1.)*

---

# PART II — LANGUAGE FOUNDATIONS

## §4. Sol Contracts (normative) — 🔒 LOCKED (2026-07-26; amendments via Decision Log only)

**Envelope:**
```json
{
  "sol": "1",
  "imprint": "declared.id@v | ~<structural-hash> | (absent: derivable)",
  "body": { "<key>": { "k": "data|var|fn|flow|sol|list", "...": "..." } }
}
```
- `sol` — language version. `body` — KV working memory; pointers target keys.
- SolValue kinds: `data`, `var` (path ref), `fn` (named callable), `flow` (nested pipeline), `sol` (nested contract), `list`.
- Plain JSON literals in `body` = sugar for `data`.

### §4.1 Imprint discipline — 🔒 LOCKED

1. **Two imprint tiers; "missing" does not exist.** Imprints are `declared` (registered `id@version`) or `structural` (system-derived, written `~<hash>`). Every materialized Sol has a structural imprint derivable on demand; absence of the field in memory is permitted, absence of *identity* is not.
2. **Declared imprints are mandatory at:** (a) persistence to Aelio DB; (b) both directions of a `Call` boundary — the registry entry (§10) declares them; (c) Model-class op outputs, which validate against declared imprints **only** (self-derived structural conformance of LLM output is void and forbidden — load-bearing for G4); (d) ~~Park continuation bag snapshots~~ **AMENDED 2026-07-26 (Amendment #3):** park snapshots carry *structural* imprints — park/resume crosses no semantic boundary (same program, point, tenant); integrity via `continuation_hash` (App G/I). Declared imprints remain mandatory at (a)–(c). Intra-flow intermediates may remain anonymous indefinitely.
3. **Structural imprint = canonical recursive hash** over the materialized body: sorted keys; fundamental-type signatures; nested `sol` hashed recursively; homogeneous lists as `list<T>`, else `list<mixed>`. `var` values are resolved before hashing. Hard dependency: canonical serialization (§4.3/A4.4).
4. **Program-bearing bags:** any Sol containing `fn` or `flow` values has **no structural imprint** and may not be persisted, converted, or passed across a `Call` boundary. Programs travel as programs (flow storage), never inside data. (Subsumes former A4.3; converters may emit `data`/`list`/`sol` only.)
5. **Warm converter applicability = `edge_id` match ∧ read-set satisfaction.** A converter's identity includes its read-set signature (the keys+types its rules touch); applicability means the incoming bag satisfies that read set. Full-shape structural imprints are lookup/bucket keys **only** — never application authority (unrelated key additions must not invalidate or misapply edges).
6. **Strict edge-only reuse (v0 — RATIFIED; edge identity AMENDED 2026-07-26, see Decision Log).** A promoted converter auto-applies only at its own `edge_id = (tenant, flow_id, producer_nid, consumer_nid)` — **nid-based, version-free**: node ids are minted once at authoring time and survive flow edits (§8.1). Any flow edit demotes **all** of that flow's edges to shadow with fast-track re-promotion (rules pre-filled, evidence preserved under stable nids; canary only). A read-set/structural match at any other edge is a **proposal hint**: rules pre-filled, LLM skipped, full gate re-entry (§16). No cross-edge or cross-flow auto-apply. Revisit only with §21 data.
7. **Imprint registry:** tenant-scoped + `vendor` namespace for stock imprints. Entry: id, version, required keys+types, optional keys, open/closed flag, default sensitivity. LLM-proposed imprints (KV-bag vision, §8.4 of synopsis) enter through the promotion gate like all learned artifacts.
8. **Imprint versioning:** any change to required keys or their types ⇒ new version. Adding optional keys = in-place minor rev, legal for open maps only. A version bump auto-demotes every dependent converter to shadow (§16.5).

### §4.2 Bag write discipline — 🔒 LOCKED
*(Resolves synopsis Open Lock #5.)*

1. **The bag flows; ops edit their write set.** One flowing bag; every op's instruction statically determines its write set (`target`/`into` paths). Execution replaces exactly those values; all other keys pass untouched. No other mutation channel exists.
2. **No implicit merge, ever.** Combining Sols is explicit Compute `merge(left, right, into, on_conflict: error|left|right)`, default `error`.
3. **`Const` is the sole whole-bag writer** (fixed emit; ingress/fixtures). Injecting into an existing bag is an edit, never Const.
4. **Single Call semantics (RATIFIED): `Call(id, args, into)` — `into` mandatory; root is not a legal `into` target.**
   - Callee input is constructed **only** from `args` (least-privilege projection; the callee never sees the caller's bag) — load-bearing for G5 and §A17.2.
   - Result lands at `into` as a nested `sol` value: callee output is namespaced; caller-key collision is impossible.
   - Read set = paths named in `args`; write set = `{into}` — both static.
   - The value at `into` is validated against the registry's declared output imprint (§4.1.2b) **before** commit; failure ⇒ `Shape`, no write.
   - `Pipe`-style stage sugar is P2 surface syntax compiling to edits; kernel semantics stay singular.
5. **Paths in instructions are literals.** No runtime-computed paths in `args`/`target`/`into`. Dynamic element access is served by `scope`/`at`/`Map` only. (Static read/write sets are a G2 + wave prerequisite.)
6. **Op writes are atomic**: full write set or nothing. On `Err` the bag is exactly pre-op; `Try`/`Fallback` handlers always see clean state.
7. **`Let` bindings are lexically scoped**: visible in `body`, shadowing permitted, unwound at exit; persistence past `Let` is an explicit write inside the body.
8. **Type-stable keys**: same-fundamental-type overwrite is a legal edit; type-changing overwrite raises `Type` unless the op is an explicit `cast`.
9. **No implicit deletion**: keys die only by explicit `drop`/`keep`.

### §4.3 Canonical serialization — 🔒 LOCKED

UTF-8; map keys sorted bytewise by code point; zero insignificant whitespace; hash = **BLAKE3** over canonical bytes. Strings: raw code points, minimal escape set, **no Unicode normalization** (hash exact bytes; normalization silently alters user data). Integers: i64, plain decimal, no leading zeros. Floats: IEEE-754 f64, shortest round-trip form, always carrying a decimal point (`2` int ≢ `2.0` float — Sol distinguishes what JSON doesn't); `-0.0` → `0.0`. **`NaN`/`±∞` are forbidden in Sol data** — rejected at ingress; any Compute op producing them raises `Type` (they poison equality, hashing, idempotency, replay). `var` resolves before canonicalization (§4.1.3); program-bearing bags have no canonical form (§4.1.4).

### §4.4 Limits — 🔒 LOCKED

v0 defaults — deployer-tunable downward, hard-capped upward: max nesting depth **32**; max keys per map **1,024**; max canonical body **1 MiB**; max list length **10,000**. Any `all`/`column`-scope op additionally requires its own `max_items` (no unbounded iteration anywhere; feeds G1). Violations raise `Budget.Size` (sub-code registered in §11).

**Attack list:**
- [x] A4.1 **RESOLVED → §4.1.** Two-tier imprints; declared mandatory at boundaries; structural derivable; edge_id + read-set as sole warm authority; strict edge-only v0.
- [x] A4.2 **RESOLVED → §4.2.** Edit-only model; single Call semantics with mandatory `into`; Const sole replacer; literal paths; atomic writes.
- [x] A4.3 **RESOLVED → §4.1.4.** Program-bearing bags cannot cross boundaries; converters never emit `fn`/`flow`.
- [x] A4.4 **RESOLVED → §4.3.**
- [x] A4.5 **RESOLVED → §4.4.**

## §5. Type system — 🔒 LOCKED (2026-07-26)

**Fundamental types:** `null | bool | int(i64) | float(f64) | str | list | map` (+ nested `sol`). NaN/±∞ forbidden (§4.3). **All Compute arithmetic is checked: overflow raises `Type`** — never wraps, never saturates.

### §5.1 Honesty clause
Typecheck is **structural**: it proves shape, type, instruction-set closure, and boundedness. It **cannot prove meaning.** No artifact is promoted on typecheck alone; semantic risk is handled exclusively by the §16 gate. Any design treating "typechecked" as "safe" is invalid by construction.

### §5.2 Cast allow-matrix (normative)
**Governing principle: `cast` changes representation only. Meaning-bearing mappings (truthiness, 0/1-as-bool, "yes"→true, case folding) are semantics and belong to `map_enum`/`default`/`const_set`, where the §16 gate scrutinizes them.**

| Category | Casts | Behavior |
|---|---|---|
| **TOTAL** | `int→str`, `float→str`, `bool→str` (`"true"`/`"false"`), identity | Always succeeds |
| **CHECKED** | `str→int`, `str→float` (strict grammar: optional sign + digits; no whitespace/underscores/NaN/inf literals) · `str→bool` (exactly `"true"`/`"false"`) · `int→float` (fails unless exactly representable in f64) | Fail-loud: `Type` (Compute) / `Convert.RuleFail`→`on_parse_fail` (conversion) |
| **MODE-REQUIRED** | `float→int` — requires `mode: trunc\|floor\|ceil\|round`; fails out-of-i64-range | Plan-time reject if mode absent |
| **FORBIDDEN** | `bool↔int`, `int→bool`, truthiness casts, `null→X` (use `default`), `X→null` (use `drop`), scalar↔`list`/`map`/`sol` (use `wrap`/`unwrap`/`path_copy`) | No path exists; plan-time reject |

`list<A>→list<B>` is not a cast form: it is an element cast under `scope: all` (mandatory `max_items`, §4.4).

**Consequence (by design):** an LLM-proposed converter cannot express semantic judgment through `cast`; all meaning-bearing mappings are funneled into the three rule ops the promotion gate audits hardest.

### §5.3 Imprint declarations (completes §4.1.7)
Declarations may reference other declared imprints for nested `sol` values (`key: sol<other.imprint@N>`); reference cycles are plan-time rejected. `extra_key_policy` (on conversion targets): `reject` (closed maps — default) | `pass` (open maps: unknown keys copied untouched) | `drop` (open maps: unknown keys removed). `pass` is legal only when the target imprint is open.

### §5.4 Refinements
Format refinements (phone, ISO datetime, …) are **validator Compute ops only** in v0 — no refinement types in the type system. First-class refinements are v2+.

**Attack list:**
- [x] A5.1 **RESOLVED → §5.2** (RATIFIED: `int→float` CHECKED with no lossy escape — misfit values indicate upstream schema error).
- [x] A5.2 **RESOLVED → §4.1.7 + §5.3.**
- [x] A5.3 **RESOLVED → §4.3 + §5 header** (i64/f64, NaN/∞ ban, checked arithmetic).
- [x] A5.4 **RESOLVED → §5.4** (validators-only v0).

## §6. Pointer & scope semantics — 🔒 LOCKED (2026-07-26)

### §6.1 Path grammar
`path = segment ("." segment | index)*` · `segment = identifier | "[" quoted-string "]"` · `index = "[" int "]"`. Identifiers `[A-Za-z_][A-Za-z0-9_]*`; quoted-bracket form for non-identifier keys (`["user-id"]`) so no data key is pointer-unreachable. Max depth 32 (§4.4). All instruction paths are literals (§4.2.5). Full EBNF in Appendix A of the conformance suite.

### §6.2 Read/write sets & aliasing
Plan time computes every op's **read set** (paths in `args`/`pred`/sources) and **write set** (`target`/`into`) from literal paths. Overlap is **prefix-aware**: a write to `P` covers all descendants of `P`. v0 executes sequentially but records R/W sets per op from day one. **Wave-legality rule (normative now, exercised in P2):** ops may share a wave iff W₁∩(R₂∪W₂)=∅ and W₂∩R₁=∅ (prefix-aware intersection). Waves become an executor change, never a spec change.

### §6.3 One privilege model (Map/Filter unification)
`Map(over: path, imports?: {key: parent.path}, body, into, max_items)`. The child's bag **is** the element; `this` = element; the child cannot see or write the parent bag. Parent context arrives only via `imports` — explicit read-only projections (RATIFIED: imports-only; no ambient parent reads). Results collect at parent `into`. **Consequence: Map children, Call targets, and Model prompts share one privilege shape — explicit projection in, declared landing path out. Ambient state never crosses any boundary.** `Filter` child = pure predicate over the element: read-only, no writes, emits `bool`. `Tee` side branches follow the same discipline: declared side-target write subtree, disjoint from the main continuation's read set.

### §6.4 `on_missing: skip` is trace-visible, never bag-visible
Skip ⇒ op becomes identity (no write) + `skipped(op_serial, path)` marker appended to the turn ledger. In `Map`, a skipped element is omitted from `into` with the same marker. Skip never fabricates; `default` is the sole fabricator and declares itself in the instruction.

### §6.5 No normative list home
`body.items` demoted to non-normative convention; `Map(over:)` targets any list path. *(Resolves synopsis Open Lock #1.)*

**Attack list:**
- [x] A6.1 **RESOLVED → §6.1.**
- [x] A6.2 **RESOLVED → §6.2.**
- [x] A6.3 **RESOLVED → §6.3** (RATIFIED: imports-only).
- [x] A6.4 **RESOLVED → §6.4.**
- [x] A6.5 **RESOLVED → §6.5.**

---

# PART III — KERNEL

## §7. Operation classes & taxonomy — 🔒 LOCKED (2026-07-26)

Five classes — Control, Compute, I/O, Model, Tool. Every data-facing step is **fixed emit** (`Const`) or **transform**; Control may additionally `Err` or `Park`. `Call` is the bridge; domain never enters the kernel.

**Classification decision procedure *(A7.1)*:** does it alter execution order/scope? → Control (kernel-eternal). Pure function of input Sol? → Compute. Touches Aelio DB? → I/O. Touches an LLM? → Model. Touches anything else external? → Tool. Not clearly one of these? → it is a *registered Call target*, never a kernel op — the anti-noun-creep rule (`NormalizePhone` is a registered validator, forever).

**`Convert` placement *(A7.2)*:** Convert is a **planner mechanism**, not a user-authorable op — planner-inserted at edges, graph-backed, rule set per §14.

## §8. Control op catalog (normative semantics) — 🔒 LOCKED (2026-07-26, all 17 ops)

**L0-A:** `Const, Identity, Call, Seq, Branch, Loop, Try, Fallback, Guard, Budget, Timeout, Once, Park, Let, Tee, Map, Filter`. Forbidden: `Sleep`; `Loop`-as-human-wait (use `Park`).

### §8.1 Node identity — 🔒 LOCKED
Every op node carries a **`nid`** minted once at authoring time and preserved across edits — identity is neither position nor content. Ledger entries, learned edges (§4.1.6 as amended), skip markers, and trace records all key on nids. Flow edits mint nids for new nodes only.

### §8.2 Batch 1 — structural ops — 🔒 LOCKED

- **`Const(v)`** — sole whole-bag writer (§4.2.3). `v` must be program-free (no `fn`/`flow`; Const mints data, §4.1.4). Literal `v` charged against §4.4 limits at plan time.
- **`Identity`** — emits input unchanged; write set ∅; conformance test #1.
- **`Seq(steps[])`** — steps see prior writes; `Err` aborts remaining steps and propagates. **`Seq` is not a transaction**: per-op atomicity (§4.2.6) means a failed Seq leaves earlier writes; recovery scope = enclosing `Try`, which sees partial state. **All-or-nothing idiom (RATIFIED — no transaction op in v0):** stage writes into a scratch subtree; final step `path_copy` staging-root → real home (single op ⇒ atomic). Revisit only if golden flows prove the idiom unbearable.
- **`Let(bindings, body)`** — bindings evaluate in declaration order; later see earlier; binding `Err` propagates without entering `body`; scoping per §4.2.7.
- **`Branch(pred, then, else?)`** — `pred` is a **pure predicate**: read-only, emits `bool`, write set ∅ — the single predicate concept shared with `Filter` children (§6.3) and `Loop.while`/`Guard.invariant`. Non-bool ⇒ `Type`. Absent `else` = `Identity`.
- **`Tee(body, side)`** — sequential v0: `body` completes, then `side`. `side` failure ⇒ `side_failed` ledger report; main path continues. `side` write set = declared side-target subtree; plan-time reject if it intersects any subsequent op's read set. Fire-and-record, provably unable to contaminate main dataflow. *(Resolves A8.5.)*

### §8.3 Batch 2 — iteration & error ops — 🔒 LOCKED (2026-07-26)

- **`Loop(while, body, max_iter)`** — `while` = pure predicate, checked before each iteration; iterations sequential, see prior writes. **`max_iter` exhaustion raises `Budget.Iter` — never a silent exit** (silent caps turn runaway bugs into quietly-wrong output; intended finite iteration belongs in the predicate).
- **`Map`** — semantics per §6.3. Element order preserved; sequential v0; wave-parallelizable in P2 by construction. **Fail-fast + atomic commit:** element error aborts the Map (`cause` records element index); since `into` is Map's sole parent write and op writes are atomic (§4.2.6), no partial list ever lands. Partial tolerance = author wraps child in `Try`; the kernel never decides partial data is acceptable.
- **`Filter(over, pred, into, max_items)`** — pure predicate per element; non-bool ⇒ `Type`; order preserved; atomic `into` commit.
- **`Try(body, catch{prefix→Instruction}, err_into, finally?)`** — caught `err.v1` is written at `err_into` (static part of Try's write set) before the handler runs; handlers inspect via ordinary pointers (§11.4). `finally` runs on success and on handled/unhandled error. **Original-error-wins:** a `finally` error during error propagation becomes a ledger report; the original propagates. On the success path a `finally` error propagates normally.
- **`Fallback(steps[])`** — advances on **step-originated** errors only, distinguished by raising nid; scope-originated errors (enclosing `Budget`/`Timeout` tripping mid-step) propagate immediately — an exhausted budget is not a failed alternative. All steps fail ⇒ last error propagates, priors chained in `cause`. Catching `Policy` is not a bypass: every effectful Call carries its own gate (§A10.3); no unpoliced alternative exists.
- **`Guard(invariant, body, on_violation?, check: entry|exit|both|each = both)`** — `invariant` = pure predicate. `each` re-checks after every op in the body subtree (ledger-visible; opt-in, O(ops)) for invariants worthless at borders only. Violation raises `Guard.Violation` carrying checkpoint (`entry`/`exit`/nid); `on_violation` is sugar for an enclosing catch of exactly that code. Resume-time re-check → batch 3. *(Resolves A8.6.)*

### §8.4 Batch 3 — resources, effects, suspension — 🔒 LOCKED (2026-07-26)

- **`Timeout(body, ms)` / `Budget(body, calls?, tokens?, ms?)`** — **`ms` meters measure active execution time only; suspended across `Park`** (calendar deadlines are `Park(until: ttl)`'s job). `calls` counts every Call execution; `tokens` from Model-class ledger entries; nested budgets all charge, first trip raises. Budget counters ride the Park continuation (per-flow budgets span turns); ambient per-turn budget (Sense) is separate. `ms` trips are wall-clock ⇒ **trip events are ledgered; replay honors the ledger, never re-measures.** Deadlines propagate into Call adapters (adapter contract: must honor deadline) so Timeout reaches inside hanging external calls.
- **`Once(body, idem_key?)`** — default `idem_key = BLAKE3(tenant, flow_instance_id, nid, canonical(read-set projection))`: once per instance, per site, per distinct input (corrected input re-executes; duplicate submit doesn't). Override: template with `var` values (data, not paths) for cross-instance dedup. Ledger protocol intent→execute→result. **At-most-once: recovery finding intent-without-result = unknown outcome ⇒ raise `Internal`, never silently re-execute.** *(Resolves A8.4.)*
- **`Call(id, args, into)` — boundedness contract:** pure/compute targets declare a cost envelope; external targets declare deadline-compliance; registered flows as targets must pass plan-time boundedness, and **the call graph over registered flows must be acyclic — recursion forbidden in v0** ("continue later" is `Park`, not recursion). **Termination theorem (G1):** acyclic call graph + every loop capped + every list op capped + every external call deadlined ⇒ every program terminates. *(Resolves A8.3.)*
- **`Park(until: event|ttl|instant, into?)`** — wake payload lands at `into` as ordinary Sol (`Express → Park(into: reply)` is the ask-and-wait idiom). Continuation: resume-nid, bag (declared imprint, §4.1.2d), budget counters, Try/Guard stacks. **On resume, before the body continues:** Policy re-check; **all enclosing Guards re-evaluate regardless of `check` mode** (the world moved); Sense refreshes.
- **Reserved `sense` subtree:** runtime-owned, rewritten at every turn entry including resume; ops plan-time rejected from writing under it. Designs out the stale-context bug class (A22.3) at the language level.

**Park-inside-X matrix** *(resolves A8.2)*:

| Park inside… | Semantics |
|---|---|
| `Seq` | Suspend at Park node; resume continues after it |
| `Map` | Element k parks → Map suspends; 0..k−1 held; resume at k. **Park-containing Map bodies are permanently non-wave-parallelizable** (plan-time detectable) |
| `Try` | `finally` does NOT run at park; context restores on resume; catch works on resumed errors |
| `Loop` | Parks mid-iteration; counter rides continuation |
| `Budget`/`Timeout` | Counters persist; `ms` suspended |
| `Guard` | Invariants re-checked on resume |
| `Let` | Bindings ride the bag snapshot |
| `Tee.side` | **Plan-time forbidden** (a suspending side effect contradicts fire-and-record) |
| Any predicate position | **Plan-time forbidden** (predicates are pure; Park is not) |

**Abandoned parks get a termination turn:** GC or instance kill executes one final ledgered turn running the enclosing `finally` chain — author cleanup executes even when the user ghosts; without this, `finally` is a lie for any parking flow.

**Attack list:**
- [x] A8.1 **RESOLVED — all 17 entries complete (§8.2–§8.4).**
- [x] A8.2 **RESOLVED → §8.4 matrix.**
- [x] A8.3 **RESOLVED → §8.4 Call entry (termination theorem).**
- [x] A8.4 **RESOLVED → §8.4 Once entry.**
- [x] A8.5 **RESOLVED → §8.2 Tee entry.**
- [x] A8.6 **RESOLVED → §8.3 Guard entry.**

## §9. Compute instruction catalog — 🔒 LOCKED (2026-07-26)

Closed v0 catalog; all pure (§A9.2), checked arithmetic (§5), mandatory limits on list producers, per-op conformance vectors alongside §8's:
- **Numeric:** `add, sub, mul, div (÷0 ⇒ Type), mod, abs, min, max` — i64/f64 per §5, no mixed-type arithmetic without explicit cast.
- **Compare/logic:** `eq, ne, lt, le, gt, ge, and, or, not` — comparisons type-strict (int vs float comparison requires cast).
- **String:** `concat, length, contains, starts_with, ends_with` (split/case ops deliberately absent — see §14 admission rule rationale).
- **Structure:** `pull (path get), exists, merge (§4.2.2 — on_conflict mandatory, default error), drop, keep, path_copy`.
- **List:** `count, append, first, last, slice (bounded), list_contains` — producers carry `max_items`.
- **Validate:** `is_type, matches_format(validator_id)` — validators are registered targets (§5.4).
- **Hash:** `blake3` over canonical form (§4.3).
- **L0-C (ledgered nondeterminism):** `now, uuid, random` — values recorded per §12.2, replayed per §12.3.

*(A9.1, A9.2 resolved.)*

## §10. Call bridge & registry — 🔒 LOCKED (2026-07-26)

**`Call(id, args, into)`** (§4.2.4) invokes registered Compute/I/O/Model/Tool targets. Domain formatters are registered targets, never kernel nouns.

### §10.1 Registry entry (normative)
`{id, version, class, input_imprint, output_imprint, boundedness (cost envelope | deadline-compliant | registered-flow), effect_class (pure | read | write | external), policy_tags, tenant_scope, origin (tenant | vendor)}`. Flows **pin** target versions; a new target version re-canaries dependent flows (§25). *(A10.1, A10.2.)*

### §10.2 Executor-enforced effect invariants (G3)
`write`/`external` Calls structurally require: policy gate evaluation, intent/result ledgering (§12.4), signature verification where declared. Plan-time reject if a `write`/`external` Call lacks its policy tags — an executor invariant, never a flow-author convention. *(A10.3.)*

### §10.3 Model-class targets — prompt discipline
**Prompts are registered artifacts, never free strings.** A Model target's registry entry additionally pins: `{template_id@version, declared slots (name + type + sensitivity), model_id@version, params (temperature etc.), output_imprint, exemplars (part of the template)}`.

1. **Slots fill only from `args`** — §4.2.4's least-privilege projection is the prompt-injection perimeter: nothing ambient (no session, no tenant state, no prior turns) reaches a prompt unless the instruction names it. The audit for "what can reach this LLM" is reading one `args` object.
2. **Composition is deterministic:** system layers (tenant personality, policy preamble, task template) are each versioned components; the composed prompt's hash is ledgered per call (§12.2 Model entries carry `prompt_hash`, matching §13's converter metadata).
3. **Output validates against the declared imprint only** (§4.1.2c) — parse-or-reject; `Model.Parse` retry cap 1 (§11.3); refusal → `Model.Refuse`; the LLM output is ledgered, replay never re-calls (§12.3).
4. **Prompt edits = new template version.** Flows pin template versions; a template version bump **demotes to canary every learned artifact whose evidence was gathered under the old template** (pathway prototypes, judge verdicts) — evidence is only valid under the prompt that produced it.
5. **Slot sensitivity** obeys §15 masking: a `pii` slot receives shape-preserved masked values unless the target's policy tags explicitly authorize raw (reviewed-tier decision).

**"The correct prompt always goes in" is thus a structural property, not a hope:** wrong prompts can't be composed (versioned components + pinned composition), wrong data can't leak in (args-only slots), wrong output can't leak out (imprint validation), and prompt drift can't silently invalidate learned behavior (version-bump demotion).

### §10.4 I/O-class targets — query discipline (added on user direction)
**Queries are not strings either.** Aelio DB reads/writes are expressed one of two ways, both closed:
1. **Registered I/O targets** — prepared, parameterized operations (the common case: `State.Read`, Sense hydrate, Recall, conversion-graph lookup), parameterized from `args`, results landing at `into` under declared imprints.
2. **Prism** for flexible reads — the closed, Sol-shaped multimodal AST implemented by `aelio-query`: `{from, match[], where[], select, limit, into?}`. `from`, match/where columns, and `select` entries are literal and collection-schema checked. `select` is mandatory and non-empty; `limit` is mandatory and bounded. The v1 match set is `key | text | vector(vector|embed) | graph | fusion(rrf)`, at most one of each ranked/graph modality; combining ranked modalities requires exactly one explicit fusion clause. `graph` carries mandatory `max_depth`, `max_nodes`, and bounded seeds. `where` is a flat AND of scalar `{col, op:eq|ne|gt|ge|lt|le, value}` predicates. No joins, subqueries, expressions, arbitrary query text, ordering, cursor paging, table dump, or model-computed field name exists in v1. The former single-modality `QueryAst` is internal authoring sugar and MUST lower to Prism before storage/execution. When Prism is invoked through `Call`, `into` is mandatory and root is forbidden; the pure database boundary may omit it.

Consequences by construction: no injection surface (no query text exists to inject into); deterministic replay (`read_result` records args hash + output and replay injects the recorded output, App G); plan-time analyzability (collection, columns, projection, bounds, and embedding/model requirements are static, so hydration needs are derivable per flow); tenant isolation (collection registry entries and physical lookups are tenant-scoped, §17.2 applies to every lookup); bounded execution (Prism limits and graph budgets feed §8.4).

## §11. Error model — ReasonCode taxonomy — 🔒 LOCKED (2026-07-26)

### §11.1 Closed taxonomy (top-level; namespaced free-text detail)
`Shape` · `Type` · `Missing` · `Budget.{Calls, Tokens, Ms, Size, Iter}` · `Timeout` · `Policy` · `Guard.Violation` · `Tool.{Transient, Permanent, Auth, RateLimit}` · `Model.{Parse, Refuse}` · `Convert.{NoEdge, RuleFail}` · `Internal`

`Budget.Iter` = `Loop.max_iter`/`max_items` exhaustion. `Once` hitting a recorded execution is **success with recorded result**, never an error.

### §11.2 Catch matching
`Try.catch` keys are code prefixes (`Tool` catches all `Tool.*`); deepest-prefix wins; no wildcards beyond prefix semantics.

### §11.3 Retryability (normative property of the code)
Retryable: `Tool.Transient` · `Tool.RateLimit` (backoff-required) · `Timeout` · `Internal` (cap 1) · `Model.Parse` (cap 1). Never retryable: all others — explicitly `Tool.Auth` (re-auth is a flow, not a retry) and all `Budget.*` (retrying budget exhaustion is self-defeating).

### §11.4 Errors are Sols — imprint `err.v1`
`{code, detail (never load-bearing for matching), op_serial, edge_id?, retryable (derived), cause? (nested err.v1)}`. Handlers inspect errors with ordinary pointer ops; the `cause` chain provides provenance for the §26 trace viewer.

### §11.5 The kernel never auto-retries (RATIFIED)
Each op executes exactly once per plan. Retryability metadata is advisory — consumed by `Try`/`Fallback` today, by `Retry` sugar (compiling to `Try`) in P2. Rationale: kernel auto-retry breaks replay determinism unless every attempt is ledgered, double-spends budgets invisibly, and hides failure frequency from §21 metrics that promotion feeds on.

**Attack list:**
- [x] A11.1 **RESOLVED → §11.1** (closed + namespaced detail).
- [x] A11.2 **RESOLVED → §11.3.**
- [x] A11.3 **RESOLVED → §11.4.**

## §12. Execution model — determinism, waves, replay — 🔒 LOCKED (2026-07-26)

### §12.1 Evaluation order
Strict, depth-first, left-to-right over the op tree; the Control tree is the sole ordering authority. v0 sequential. Waves pre-decided: §6.2 legality rule + day-one R/W recording *(A12.2 resolved)*.

### §12.2 Turn ledger
Append-only per flow instance; entries `{seq, turn_id, nid, kind, payload, payload_hash}`. `kind` enumerates every nondeterminism source already mandated by prior locks: `Now`/`Uuid`/`Random` · Model outputs · Tool results · hydrated `sense` snapshots · pathway picks + score vectors · `Budget.ms`/`Timeout` trips · `Once` intent/result · skip markers · `side_failed`/finally-failure reports · `Guard.each` results · termination turns.

### §12.3 Replay rule (G2)
Given program P (pinned versions), initial bag B, ledger L: execution is a pure function. Replay consumes entries in `seq` order instead of performing effects — never calls LLM/tools, never reads a clock *(A12.3 resolved)*. **Divergence is a hard refuse** (nid/kind mismatch ⇒ stop with diagnostic; no best-effort resync — divergence means version drift or corruption).

### §12.4 Crash windows — universal effect accounting (RATIFIED)
The intent→execute→result ledger protocol applies to **every `write`/`external`-classed Call**, not only `Once`-wrapped ones; recovery finding intent-without-result ⇒ fail-loud unknown outcome (§8.4). The kernel guarantees at-most-once *accounting* for all effects; `Once` is purely the dedup layer above it. `read`-class and Model calls are result-ledgered only (re-execution on recovery wastes money, corrupts nothing).

### §12.5 Persistence & continuations
Ledger appends are the WAL; bag persisted at turn end and at Park. Continuation serialization pins kernel version; resumable only on the pinned version pending §25 migration doctrine *(A12.1 resolved; migration half deferred to §25 where it belongs)*.

**Attack list:** all resolved — A12.1 → §12.5/§25 · A12.2 → §12.1 · A12.3 → §12.3.

---

# PART IV — CONVERSION SUBSYSTEM

## §13. Conversion graph data model — 🔒 LOCKED (2026-07-26)

**Edge identity:** per §4.1.6 as amended — `edge_id = (tenant, flow_id, producer_nid, consumer_nid)`; nid-based, version-free; flow edits demote all flow edges to shadow with fast-track re-promotion.

**Metadata per edge:** `conversion_id, version, tenant_id, status, from_signature, to_digest, rules, type_map, extra_key_policy, on_parse_fail, proposed_by, prompt_hash?, evidence, sensitivity`.

### §13.1 Status lifecycle
`proposed → shadow → canary → promoted → demoted → retired`, plus **`rejected`** (failed structural check; kept with reason + rules-hash so repeat proposals short-circuit to backoff — no proposal loops). Transitions: `proposed→shadow` automatic on structural pass; `shadow→canary`, `canary→promoted` on §16 evidence thresholds; `demoted` (→shadow) on attribution failures, imprint version bump (§4.1.8), flow edit (§4.1.6), or manual (always available, always cheap); `retired` on manual action, N demotions, or long-unused window.

### §13.2 Evidence schema
`{shadow_runs, shadow_agreements, canary_runs, canary_successes, downstream_failures_attributed, distinct_input_hashes, last_validated_at, validation_method}`. **All §16 thresholds are defined over `distinct_input_hashes`, never raw run counts** — evidence cannot be inflated by repetition.

**Attack list:** all resolved — A13.1 → §4.1.6 amendment · A13.2 → §13.1 · A13.3 → §13.2.

## §14. Conversion rule ops (closed set) — 🔒 LOCKED (2026-07-26)

**Set:** `rename, drop, keep, default, cast, wrap, unwrap, map_enum, path_copy, const_set, trim`. Rules execute **sequentially as written**; each rewrites the working Sol; no rule may reference the rule list itself (no loops/self-reference — the set is non-computational, A14.4, proven by construction: no rule's output feeds rule *selection*; conformance suite carries the property test).

**Per-rule entries:** `rename(from,to)` — from must exist else `RuleFail`. `drop(path)` / `keep(paths[])` — keep drops all others; both total. `default(path, v)` — writes v iff path absent; **fabricating**. `cast(path, to, mode?)` — per §5.2 matrix; failures → `RuleFail` → `on_parse_fail`. `wrap(path, key)` / `unwrap(path)` — single-level structure; unwrap of non-map → `RuleFail`. `map_enum(path, table)` — unmapped value → `RuleFail`, never invented; consumer never runs on unmapped data *(A14.2)*. `path_copy(from,to)` — from must exist. `const_set(path, v)` — unconditional write; **fabricating**. `trim(path)` — per admission entry.

**Fabrication escalation *(A14.3)*:** any proposal containing a fabricating rule (`default`, `const_set`) escalates its edge to **reviewed tier regardless of reach** — minting unobserved data is inherently semantic.

**`trim`:** ASCII whitespace, both ends, strings only — admitted (RATIFIED) to close the dirty-scalar repairability gap. **Rule-op admission rule (normative):** admissible only if (a) pure, (b) total or fail-loud, (c) meaning-free, (d) locale/environment-independent. **Case folding explicitly fails (c)** — case carries meaning in enums, codes, IDs; that is `map_enum`'s job. Future admissions must cite this rule in the Decision Log.

`on_parse_fail ∈ {error (default) | default(v)}` per edge; `error` propagates `Convert.RuleFail` as a step-originated error at the consumer boundary (catchable, §8.3 Fallback rules apply).

**Attack list:** all resolved — A14.1 → entries above · A14.2 → map_enum entry · A14.3 → fabrication escalation · A14.4 → non-computation by construction + conformance property test.

## §15. Cold path — LLM proposal protocol — 🔒 LOCKED (2026-07-26)

**Proposer context:** from-side structural signature + to-side consumer digest (required keys+types) + sample values per sensitivity. **Sensitivity levels (normative, shared with §28):** `public | internal | pii | secret`. Samples: raw for public/internal; **shape-preserving masked** for pii (`"+91XXXXX0903"` — format inferable, value absent); types-only for secret. Output contract: closed JSON, §14 rule ops only, parse-or-reject, size-capped.

**Single-flight per edge:** concurrent misses on one `edge_id` trigger exactly one proposal; other requests take the ordinary `Convert.NoEdge` miss path (flow's `Try` handles) — a hot edge going cold never fans out parallel LLM calls or stalls users mid-turn. Failed proposals (structural reject or gate reject) back off exponentially per edge; capped attempts per window; repeat proposals matching a `rejected` rules-hash short-circuit to backoff (§13.1).

**Containment theorem:** payload content is untrusted input to the proposer, and **no payload content can cause execution** — proposer output is only rules from §14's closed, non-computational set (A14.4 obligation, load-bearing here). The worst achievable outcome of a malicious payload is a wrong mapping — structurally downgraded into exactly the failure class §16 defends against hardest.

**Attack list:** all resolved — A15.1 → sensitivity/masking above · A15.2 → single-flight + backoff.

## §16. Promotion gate — the safety core — 🔒 LOCKED (2026-07-26)

**Premise (from §5.1):** no phase proves meaning; each phase proves one thing honestly, and the irreducibly-semantic residue is routed to humans by reach.

### §16.1 Phases
1. **Structural** — rules parse; closed set; non-computational (A14.4); type_map consistent. Proves *well-formedness*.
2. **Shadow = validate-without-consume.** Converter runs on live traffic; output checked against the to-side consumer digest and discarded; flow takes its ordinary miss path. Proves *robustness over the real input distribution* (missing enum values, format-failing casts). Needs no old path — the digest is the oracle. *(Resolves A16.1 bootstrap.)*
3. **Canary = bounded consumption with attribution.** First M eligible uses consume output; each records downstream outcome. Attribution v0 *(A16.2, deliberately turn-local)*: a use fails iff the turn raises an error whose `cause` chain passes through the consumer nid, or any downstream `Guard.Violation` same-turn. Cross-turn signals integrate when §20 lands.
4. **Promotion** — thresholds over **distinct inputs** (§13.2).

### §16.2 Sensitivity tiers — reach-based (AMENDED during ratification)
| Tier | Classification | Gate |
|---|---|---|
| **auto** | no `write`-class or external tool Call reachable downstream of the consumer nid | thresholds only |
| **reviewed** | `pii` edge, **or any `write`/external Call reachable downstream of consumer** (static taint walk — computable: plans static, paths literal, effect classes declared, call graph acyclic) | thresholds + explicit deployer approval (rules + sample before/after transforms) |
| **locked** | `secret` | never auto-proposed; deployer authors or explicitly invites |

**Carve-out:** expression-to-current-user is not "reach" — otherwise every conversational edge is reviewed, approval fatigue sets in, and a rubber-stamped tier is worse than none. **Founding-bug check:** `active→loggedin` reaches `SendOtp` (external) ⇒ reviewed ⇒ human sees the rename before it governs auth.

### §16.3 Advisory semantic review
Independent LLM judge pass on auto-tier mappings; recorded as `validation_method: semantic_review` (verdict + rationale). **Advisory only:** judge-flagged ⇒ held at canary pending review; judge-approved still needs thresholds. LLM opinion accelerates and warns; it never promotes.

### §16.4 v0 constants (deployer-tunable within bounds)
Shadow→canary: ≥20 distinct inputs, ≥95% digest-validation. Canary→promoted: ≥20 distinct canary inputs, ≥98% downstream success, zero attributed Guard violations. Canary window M=50. **Demotion:** attributed failure >2% trailing, or any single attributed `Guard.Violation` ⇒ immediate demote to shadow (evidence retained, fast-track re-canary). Three demotions in window ⇒ retired, manual revival.

### §16.5 The gate is generic *(resolves A16.4)*
One parametrized artifact gate — phases, evidence, tiers, lifecycle identical — for converters, pathways, procedures, LLM-proposed imprints; only the **agreement predicate** varies per class (converter: digest validation; pathway: retrospective outcome match; procedure: shadow-propose-while-executing-old). §19 supplies a predicate, not its own safety machinery.

### §16.6 Mutation harness (RATIFIED — committed P1 deliverable, §27)
Generate deliberately-wrong converters (swapped mappings, plausible wrong renames, inverted enums); run the full gate against recorded traffic; measure and **publish catch rate per phase** in the repo. The open-source claim becomes "the gate caught N% of mutated converters; here's the harness, run it yourself" — credibility argument and regression suite in one.

**Attack list:** all resolved — A16.1 → §16.1–2 · A16.2 → §16.1(3) · A16.3 → §16.4 · A16.4 → §16.5.

## §17. Multi-tenancy & poisoning defenses — 🔒 LOCKED (2026-07-26)

**Ratified defaults:**
1. **No learned artifact ever crosses a tenant boundary.** Cross-tenant sharing, if ever, is an explicit deployer-signed export/import — never automatic.
2. All lookups keyed by `tenant_id` first; a missing tenant key is `Internal`, never a fallback to global.
3. Cold-path prompts never mix tenants' data.
4. Rate/size quotas per tenant on proposals and stored artifacts.

**Attack list:**
- [x] A17.1 **RESOLVED:** stock artifacts are `origin=vendor` class — version-pinned, gate-exempt on install, demotable like any artifact (§A17.1 proposal ratified with §16 package).
- [x] A17.2 **RESOLVED:** cold-path LLM surfaces = converter proposer (§15) + pathway prototype author + Model-class ops; containment at each = closed-JSON out, §14/§15 rule-op-only or declared-imprint-validated output (§4.1.2c), no surface may emit `fn`/`flow` (§4.1.4). Audit surface collapsed to "read the args" by §6.3 privilege unification.

---

# PART V — LEARNING SUBSYSTEM

## §18. Pathway registry & selection — 🔒 LOCKED (2026-07-26)

**Selection is a classifier and gets classifier hygiene (SemanticRoute conclusions ported):** decision requires top-1 score ≥ τ AND margin(top1−top2) ≥ δ AND entropy below ceiling; failing any one routes to a **mandatory declared fallback pathway** — every decision point declares one or plan-time reject. Fallback rate is a first-class §21 metric and the de facto drift detector. Calibration is empirical per tenant, never copied constants. **Cap: 8 pathways per decision point (v0)**; raised only with misrouting data.

**Prototype lifecycle:** embedding model pinned per tenant; model upgrade ⇒ re-embed all prototypes + affected pathways drop to canary — scores are not comparable across embedding spaces (the Lighthouse score-space bug class, made structurally impossible). Pathway picks ledger-recorded with full score vectors (§12.2).

**Gate integration (§16.5):** agreement predicate = retrospective outcome match. Shadow: selector scores the new prototype but takes the incumbent; evidence = would-fire share **+ win rate over turns where the incumbent failed** — promotion is earned by addressing observed failures, not plausibility.

**Attack list:** all resolved — A18.1 → hygiene above · A18.2 → cap 8 · A18.3 → pinned-model lifecycle · A18.4 → §12.2.

## §19. Procedure learning & promotion — 🔒 LOCKED (2026-07-26)

**A procedure is any subtree satisfying the Call-target registry contract** (declared input/output imprints, boundedness, effect classes) — **promotion IS registration.** A promoted procedure is a registered Call target, inheriting version pinning, DAG membership, policy gates, and every §10 invariant with zero new machinery. *(A19.1 confirmed.)*

**Mining trigger (v0):** ≥5 successful occurrences of an isomorphic op subsequence (same Call ids, compatible arg shapes) within a 30-day ledger window ⇒ propose. *(A19.3.)*

**Gate integration:** shadow = miner proposes while turns execute the old way, comparing predicted vs actual sequences; canary = bounded real executions with §20 attribution. *(A19.2.)*

## §20. Credit assignment / attribution — 🔒 LOCKED (2026-07-26)

**v0 scheme — deliberately dumb, honest, conservative:** flow-outcome boolean attributed to every artifact used in the flow, usage-weighted. **Documented biases:** popular artifacts accumulate attribution noise; co-occurring artifacts share blame indistinguishably. Both biases are conservative — they produce false *demotions* (cheap, per the asymmetry), never false promotions (expensive). A wrong-but-clever counterfactual scheme would err in the dangerous direction; clever is v2, after data. *(A20.1.)*

**Demotion-eligible negative signals:** cause-chain-attributed errors · Guard violations · flow abandonment within N turns of artifact use · **explicit** correction signals only. Implicit correction detection (rephrasing) is v2 — in v0 it is noise. *(A20.2.)*

## §21. Hit-rate economics & instrumentation — 🔒 LOCKED (2026-07-26)

**Metrics (persisted in Aelio DB, dashboard ships with the release):** warm-hit rate per artifact class · cold-path cost · promotion survival curves · fallback rate (§18) · mutation-harness catch rates (§16.6). *(A21.1.)*

**Pre-registered kill criteria *(A21.2)* — written before the data exists:**
- **Converters (the load-bearing bet):** warm-hit < ~60% at 90 days for an integrated tenant ⇒ the conversion-economics claim is falsified; README changes.
- **Procedures (the speculative bet, and the doc says so):** < 10% of turns touching a promoted procedure at 6 months across ≥3 active tenants ⇒ procedure learning demoted from headline to experimental.

Pre-registration is what separates an engineering claim from a pitch — and makes good numbers credible when they arrive, because the bar predates the jump. Numbers are calibrated placeholders, deployer-of-the-project adjustable pre-launch; adjustment after launch requires a Decision Log entry.

---

# PART VI — RUNTIME & PERSISTENCE

## §22. Turn loop, Sense, hydration — 🔒 LOCKED (2026-07-26)

**Turn loop:** message in → hydrate → Sense freeze → flow gate → program → persist.

### §22.1 `sense.v1` (normative fields)
`env {now, tz, turn_index, channel, timings}` · `session {open_loops, active_flow, pending_step, last_seen}` · `budget` · `tenant` · `deployer_state_ref` · `reachable_tools`. Lives at the reserved runtime-owned `sense` subtree (§8.4).

### §22.2 Flow gate — single active flow + read-only detours (RATIFIED)
Pathways declare `instantiates_flow: bool`. While a flow is pending: the selector considers only non-instantiating pathways (inline answers, zero flow state touched) plus the pending flow's continuation; a flow-requiring intent gets an Express acknowledging + deferring, recorded as an open loop in `memories` so the agent returns to it. **No flow stack in v0** — stacks are v2, gated on demand data. *(A22.2.)*

### §22.3 Hydration order (normative — the "everything is hot" bug is an ordering bug)
(1) load durable rows → (2) compute temporal/recency classifications **from loaded timestamps** → (3) freeze `sense` → (4) only then update `session.last_seen` → (5) flow gate → (6) program. *(A22.3 closed.)*

**Attack list:** all resolved — A22.1 → §22.1 · A22.2 → §22.2 · A22.3 → §22.3 + §8.4.

## §23. Park / resume semantics — 🔒 LOCKED (2026-07-26)

Core semantics per §8.4. **Wake routing:** inbound user messages route to the session's parked instance via the flow gate (§22.2). External events route via an **event key minted at Park time** — derived from (tenant, flow_instance, park nid), handed to the external system as its callback token; wake authorization = token possession, nothing guessable. **GC defaults:** event-parks 30 days; ttl-parks self-defining; instant-parks until instant + grace. Termination turns (§8.4) run under a small fixed budget — finally chains only. Continuation serialization pins kernel version (§12.5, §25).

## §24. Storage schema (Aelio DB mapping) — 🔒 LOCKED (2026-07-26, outline level; implementation DDL cites this)

**Tables:** `states` · `flow_instances` · `sessions` · `memories` · `messages`/`turns` · `ledger` (§12.2) · **unified `artifacts`** (one lifecycle + evidence schema across converters/pathways/procedures/imprints — the generic gate's data-model dividend) + per-class detail tables · `registry` (§10.1) · `rejected_proposals` (§13.1) · `metrics` (§21).

**CAS discipline *(A24.2)*:** version column on `states`/`flow_instances`; compare-and-swap on write; conflict raises `Internal` with one hydration-layer retry — conflict resolution never reaches the kernel.

## §25. Versioning & migration of stored programs — 🔒 LOCKED (2026-07-26)

**Doctrine (RATIFIED):**
1. Every artifact pins `sol` version, kernel version, per-Call target versions (incl. prompt template versions, §10.3), imprint versions.
2. Op semantic changes = new op version; executor **refuses** unknown versions — never best-effort.
3. Parked continuations resume only on their pinned kernel version, or via an explicit, tested migration function — never implicitly.
4. Imprint version bumps auto-demote dependent converters to shadow (§4.1.8).
5. **Kernel-version bumps demote all learned artifacts to canary** — cheap re-validation against new semantics catches drift the pins can't see.

## §26. Observability & debugging — 🔒 LOCKED (2026-07-26)

**v1-shipping deliverables (product, not ops sugar):** trace viewer (op-by-op bag diffs straight from the ledger) · replay CLI ("re-run turn T of tenant X locally, bit-identical") · edge inspector ("why did this Convert fire" + full evidence) · pathway explainer (score vectors, margins, fallback reasons). All read the ledger; none require instrumentation beyond what §12.2 already mandates.

## §27. Testing & conformance — 🔒 LOCKED (2026-07-26)

The §8 per-op entries **are** the conformance test vectors. Property tests: termination under budget · replay determinism · rule-set non-computation (§14/A14.4) · canonical-hash stability (§4.3). Golden flow #1: login (Appendix A). Instruction-parser fuzzing (LLM output = adversarial input). **Mutation harness (§16.6) with published per-phase catch rates — P1 committed.**

## §28. Security & policy enforcement — 🔒 LOCKED (2026-07-26, consolidation)

Every mechanism locked elsewhere; this section is the traceability page a security reviewer reads first: policy = plan-time structure check (§10.2) + runtime gate + resume re-check (§8.4) · tool signature verification (§10.2) · sensitivity contract (§15, §10.3.5) · prompt perimeter = args-only slots (§10.3.1) · injection containment theorem (§15) · tenant isolation (§17) · audit trail = the ledger (§12.2).

---

# PART VII — IMPLEMENTATION

## §29. Crate / module layout & the interpreter — 🔒 LOCKED (2026-07-26)

**The interpreter is a two-phase machine (added on user direction):**
1. **Planner** — parses instruction JSON → op tree; runs every static check the spec mandates: schema closure (§4/§5), literal paths + R/W set derivation (§6.2), boundedness incl. DAG check (§8.4), policy-tag presence (§10.2), Park-position legality (§8.4 matrix), Tee dataflow isolation (§8.2), query-AST limits (§10.4), hydration-need derivation (§10.4). A plan that passes is executable by construction.
2. **Executor** — sequential depth-first tree walk (§12.1) over the checked plan; ledger appends per §12.2; effect protocol per §12.4. Replay mode = same executor, ledger-fed (§12.3).

**Crates:** `aelio-sol` (contracts, types, paths, canonical serialization — zero internal deps) · `aelio-kernel` (Planner, Executor, ledger, replay, op catalog) · `aelio-convert` (graph, rules, gate) · `aelio-learn` (pathways, procedures, attribution, metrics) · `aelio-query` (dataset registry, query AST — §10.4) · `aelio-prompt` (template registry, composition — §10.3) · `aelio-store` (Aelio DB bindings) · `aelio-cli` (trace viewer, replay, inspectors — §26). Dependency direction strictly downward.

## §30. Build order & milestones — 🔒 LOCKED (2026-07-26)

**Spec status: all 30 sections LOCKED.** Implementation may cite any section.

- **P0:** `aelio-sol` complete + canonical hashing conformance; Planner with full static checks; sequential Executor + ledger + replay CLI (minimal); Control L0-A + Compute v0; Once/CAS; Park/resume v0 incl. termination turns; **login flow golden test green end-to-end** (Appendix A).
- **P1:** conversion graph + generic gate (shadow/canary/tiers); **mutation harness with published catch rates**; pathway registry + selection hygiene; prompt registry (§10.3); query AST (§10.4); tenant isolation enforcement; trace viewer + inspectors.
- **P2:** procedure mining + promotion; metrics dashboard + kill-criteria tracking; `Switch`/`Retry`/`Pipe` sugar (compile-down only); wave scheduling (R/W sets recorded since P0).

**Open-source milestone gate:** §2 table verified against code (every G traced to passing tests); login golden test green; replay CLI demo; mutation-harness numbers published.


---

# PART VIII — SERVER / SDK TOPOLOGY (added 2026-07-26, all RATIFIED)

## §31. Tool execution topology — SDK reverse channel — 🔒 LOCKED
**The server never holds customer tool credentials.** The server emits tool-call requests over the persistent connection to the deployer's SDK; the SDK executes in the deployer's environment with the deployer's credentials and returns the result. Structurally reuses existing locks: a reverse-channel call is `Park(until: event)`-shaped with a §23 correlation token; the §8.4 deadline contract propagates as a wire field; §12.4 intent→result ledgering is transport-agnostic. **Degradation:** disconnected SDK ⇒ `Tool.Transient` at the deadline; flows' `Fallback` handles it (§8.3). The B2B2C no-key-exposure pitch is a structural property, not a feature.

## §32. Control plane vs data plane — 🔒 LOCKED
**Control plane** (HTTP, deployer keys): SDK registers artifacts — tools, flows, imprints, prompt templates, datasets — as **versioned pushes** into the §10 registry; a push is a new version; pins + §25 doctrine apply automatically (SDK deploys inherit migration safety free). **Data plane** (one persistent WebSocket/gRPC stream per SDK connection, channel/session tokens): turns, wake events, reverse tool channel — multiplexed. **Wire protocol is itself versioned** (sweep addition): handshake negotiates protocol version; server refuses unknown majors (§25 doctrine applied to the wire).

## §33. Concurrency & scale-out — 🔒 LOCKED
Per-flow-instance actor model on tokio: one logical single-writer per instance (backed by §24 CAS; per-instance turn serialization is what makes ledger `seq` meaningful); instances multiplex freely across threads. **Per-instance inbound queue + configurable coalescence (RATIFIED):** rapid successive messages within `debounce_ms` coalesce into one turn's input, up to `max_coalesce` messages; tenant-configurable with v0 defaults `debounce_ms=1500`, `max_coalesce=5`, `queue_depth=20`; queue overflow drops-with-notice rather than unbounded buffering; coalesced input arrives as an ordered list Sol so flows see message boundaries. Scale-out = **tenant sharding**: §17.1 isolation means tenants share nothing, so horizontal scale is embarrassingly parallel; embedded Aelio DB per node fits exactly. Single-node v0; shard-by-tenant v1+; no hot-path consensus machinery, ever.

## §34. Model provider layer — 🔒 LOCKED
Provider trait in `aelio-prompt`'s calling layer (Anthropic / OpenAI / local adapters). **BYO-key from day one**, tenant-scoped, encrypted at rest. Model calls execute **server-side** — the prompt factory composes there; shipping composed prompts outward would leak template IP and add latency. §10.3's pinned `model_id@version` is the trait's contract. Hosted-cloud token/subscription model is v2/v3 product layer above this, requiring no architectural change.

## §35. Channel adapters — 🔒 LOCKED
WhatsApp/voice/web adapters live server-side as data-plane ingresses producing ordinary turns and wake events; the SDK never touches channels — its surface is exactly two things: register artifacts, execute tools. **End-user identity** (sweep addition): sessions are channel-scoped by default; cross-channel identity linking is a deployer concern exercised through their own tools writing to `states` — the platform provides the session model (§22/§24), never asserts identity equivalence itself. **Streaming posture** (sweep addition): v0 turn responses are non-streaming (WhatsApp-shaped); voice-grade streaming is a v2 adapter capability requiring no kernel change (Model output ledgering is completion-based either way).

---

# APPENDICES

## Appendix A — Worked example: login flow (normative walkthrough, golden test #1)

```text
Flow: login.v1 (all nodes carry nids; edges shown as e#)
Seq
├─ Call(io.sense_hydrate, args:{}, into: hydration_meta)          # sense subtree runtime-written (§8.4)
├─ Call(io.state_read, args:{key:"deployer_auth"}, into: auth_raw) # I/O prepared target (§10.4.1)
├─ [e1: auth_raw → pred digest]  ← Convert edge; cold-first-run learns
│    rename active→loggedin (reviewed tier: reaches SendOtp — §16.2)
├─ Branch(pred: eq(pull(auth_raw.loggedin), true),
│   then: Call(flow.authed_menu, args:{...}, into: r),
│   else: Seq                                                      # unauthorized path
│     ├─ PathwaySelect(decision_point: unauth.v1)                  # §18: τ/δ/entropy + fallback
│     ├─ Branch(pred: eq(pull(pathway.pick),"start_login"),
│     │   then: Seq                                                # the login procedure
│     │     ├─ Call(model.ask_phone, args:{lang: pull(sense.env.channel)}, into: ask1)  # §10.3 template
│     │     ├─ Park(until: event, into: reply1)                    # wake = user message via flow gate
│     │     ├─ Call(compute.validate_phone, args:{v: pull(reply1.text)}, into: phone)   # validator (§5.4)
│     │     ├─ Guard(invariant: exists(phone.e164), check: entry,
│     │     │   body: Once(                                        # §8.4: per-instance, per-input
│     │     │     Budget(calls:3, ms:20000,
│     │     │       body: Call(tool.send_otp, args:{to: pull(phone.e164)}, into: otp_send))))
│     │     ├─ Call(model.ask_otp, args:{}, into: ask2)
│     │     ├─ Park(until: event, into: reply2)
│     │     ├─ [e2: reply2 → verify digest] ← Convert; trim + cast str→int (auto tier candidates,
│     │     │    but reaches VerifyOtp ⇒ reviewed)
│     │     ├─ Call(tool.verify_otp, args:{code: pull(reply2.code)}, into: verify)
│     │     ├─ Call(io.state_write, args:{key:"deployer_auth", v:{loggedin:true}}, into: sw)
│     │     └─ Call(model.express_success, args:{}, into: out),
│     │   else: <pathway-specific subtrees: anon_actions | entertain_nudge | divert>)
```
Ledger highlights per turn: sense snapshot, pathway score vector, Model outputs + prompt_hashes, Once intent/result around send_otp, Park continuations, Convert firings + edge evidence increments. Conformance assertions: replay bit-identity; Park-resume Guard/Policy re-checks; e1 requires deployer approval before promotion; abandoned-park termination turn runs no effectful cleanup (no finally here) but GCs the instance.

## Appendix B — Traceability matrix (guarantees × threats × mechanisms)

| | Buggy LLM output | Semantic promotion | Malicious tenant | Drift | Runtime fault | Kernel evolution |
|---|---|---|---|---|---|---|
| **G1 termination** | §4/§5 parse-reject | — | §10.4 limits, §17.4 quotas | — | §8.4 deadlines | §25.2 refuse-unknown |
| **G2 replay** | §12.3 hard-refuse | §12.2 evidence trail | §12.2 audit | §12.2 prompt_hash | §12.4/§12.5 WAL | §12.5 pinning |
| **G3 policy** | §10.2 plan-reject | §16.2 reach tiers | §10.2 + §8.4 resume re-check | — | §8.4 re-check | §25.5 canary-all |
| **G4 verifiable output** | §4.1.2c, §10.3.3 | §15 containment | §15 containment | §10.3.4 | — | — |
| **G5 least privilege** | §4.2.4 | — | §6.3, §10.3.1 | — | — | — |
| **G6 tenant-safe learning** | §13.1 rejected-state | §16 full gate, §14 escalation | §17.1–4, §4.1.6 | §4.1.5 read-set, §18 fallback-rate | — | §25.5 |
| **G7 migration** | — | — | — | §4.1.8, §10.3.4 | §12.5 | §25 doctrine |

No empty load-bearing cells: dashes mark threat/guarantee pairs where the threat does not apply to that guarantee.

## Appendix C — Glossary mapping
Mother-doc section ↔ existing companion docs (`AELIO_L0_GLOSSARY`, `AELIO_SOL_CONTRACTS_KERNEL`, …) so nothing is spec'd in two places. On conflict, this document wins.

## Appendix E — Complete instruction schema (normative JSON shapes)

Every instruction node: `{"nid": "<stable-id>", "op": "<OpName>", ...op-fields}`. Unknown fields ⇒ plan-time reject. Paths are literal strings per §6.1 grammar. `[required]` unless marked `?`.

**Expression grammar (pure predicates & arg values):**
```json
Expr = {"lit": <json-value>}                       // literal (program-free)
     | {"pull": "<path>"}                          // read from bag
     | {"fn": "<compute-op>", "args": [Expr, ...]} // §9 catalog only; Planner verifies purity
```
Predicate = Expr whose result type is `bool` (Planner-checked where statically known; runtime `Type` otherwise).

**Control ops:**
```json
{"op":"Const",    "v": <SolBody>}                                        // program-free (§4.1.4)
{"op":"Identity"}
{"op":"Seq",      "steps": [Instr, ...]}
{"op":"Let",      "bindings": [{"key":"<ident>", "value": Expr}, ...], "body": Instr}
{"op":"Branch",   "pred": Expr, "then": Instr, "else"?: Instr}
{"op":"Loop",     "while": Expr, "body": Instr, "max_iter": int>0}
{"op":"Try",      "body": Instr, "catch": {"<code-prefix>": Instr, ...},
                  "err_into": "<path>", "finally"?: Instr}
{"op":"Fallback", "steps": [Instr, ...]}                                  // ≥2 steps
{"op":"Guard",    "invariant": Expr, "body": Instr,
                  "on_violation"?: Instr, "check"?: "entry|exit|both|each"}  // default "both"
{"op":"Budget",   "body": Instr, "calls"?: int>0, "tokens"?: int>0, "ms"?: int>0}  // ≥1 meter
{"op":"Timeout",  "body": Instr, "ms": int>0}
{"op":"Once",     "body": Instr, "idem_key"?: {"template": [Expr, ...]}}  // default per §8.4
{"op":"Park",     "until": {"kind":"event"} | {"kind":"ttl","ms":int>0}
                         | {"kind":"instant","at":"<iso8601>"},
                  "into"?: "<path>"}
{"op":"Tee",      "body": Instr, "side": Instr, "side_root": "<path>"}    // side writes ⊆ side_root
{"op":"Map",      "over": "<path>", "imports"?: {"<key>":"<parent-path>", ...},
                  "body": Instr, "into": "<path>", "max_items": int>0}
{"op":"Filter",   "over": "<path>", "pred": Expr, "into": "<path>", "max_items": int>0}
{"op":"Call",     "id": "<target-id>@<version>", "args": {"<slot>": Expr, ...}, "into": "<path>"}
```

**Notes binding schema to locks:** `into` may never be root (§4.2.4). Aelio DB flexible reads are not a kernel op: `Call` to pinned target `aelio-db.prism@1` projects a closed §10.4 Prism AST in `args.query`; its Call-level `into` is mandatory. The legacy single-modality `QueryAst` is compile-time sugar only. Conversion is Planner-inserted, never authored (§7). Park forbidden in `Tee.side` and all predicate positions (§8.4) — Planner rejects. `Guard.each` + `Map` legal but O(elements×ops) — Planner warns.

**err.v1 (full):** `{"code": str, "detail": str, "op_serial": "<nid>", "edge_id"?: str, "retryable": bool, "cause"?: err.v1}`

## Appendix F — Elaboration backlog (prose → implementation-grade artifacts)

Decisions are complete. The minute-detail annexes are now derived and retained as normative implementation references:

| # | Artifact | Source §§ | Status |
|---|---|---|---|
| F1 | Instruction schema (all ops, Expr grammar, err.v1) | §8, §9, §11 | ✅ Appendix E |
| F2 | Path grammar full EBNF | §6.1 | ✅ [`F2_path_grammar.md`](../annexes/F2_path_grammar.md) |
| F3 | Compute op signature table (arg types, ReasonCodes per op) | §9 | ✅ [`F3_compute_signatures.md`](../annexes/F3_compute_signatures.md) |
| F4 | Conversion rule-op JSON shapes + edge/evidence JSON | §13, §14 | ✅ [`F4_conversion_rules.md`](../annexes/F4_conversion_rules.md) |
| F5 | `sense.v1` full field types | §22.1 | ✅ [`F5_sense_v1.md`](../annexes/F5_sense_v1.md) |
| F6 | Registry entry JSON (all classes incl. Model template + I/O dataset) | §10 | ✅ [`F6_registry_entries.md`](../annexes/F6_registry_entries.md) |
| F7 | Prompt template file format (slots, layers, exemplars, composition) | §10.3 | ✅ Appendix J |
| F8 | Ledger entry payload schema per `kind` | §12.2 | ✅ Appendix G |
| F9 | Wire protocol messages (control-plane HTTP + data-plane stream frames, handshake, reverse tool channel) | §31–§32 | ✅ Appendix H |
| F10 | Aelio DB DDL per §24 table | §24 | ✅ [`F10_aelio_db_ddl.md`](../annexes/F10_aelio_db_ddl.md) |
| F11 | Conformance vector format + initial vectors for §8 entries | §27 | ✅ [`F11_conformance_vectors.md`](../annexes/F11_conformance_vectors.md) |
| F12 | Continuation serialization format | §12.5, §23 | ✅ Appendix I |
| F13 | Gate/lifecycle state-machine table (exact transition triggers) | §13, §16 | ✅ Appendix K |
| F14 | Metrics schema (fields, aggregation windows) | §21 | ✅ [`F14_metrics_schema.md`](../annexes/F14_metrics_schema.md) |

Rule: an F-item is done when Claude Code can implement from it with zero prose interpretation; each completed item flips here with a Decision Log entry only if it *changed* a decision (pure elaboration needs no ratification).

### F.1 Derivation instructions for remaining items (each: sources → output → completion check)

**F2 — Path grammar EBNF.** Sources: §6.1, §4.2.5, §4.4. Output: complete EBNF block + a table of ≥10 positive and ≥10 negative examples (incl. quoted-bracket keys, index forms, depth-32 boundary). Completion check: every path appearing in §8, App A, and App E parses; grammar structurally cannot express computed paths or root-`into`; depth >32 rejected.

**F3 — Compute signature table.** Sources: §9, §5 (matrix + checked arithmetic), §11. Output: one row per op — arg count/types, return type, raisable ReasonCodes, edge cases (÷0 ⇒ Type; overflow ⇒ Type; no op may produce NaN/∞ per §4.3). Completion check: ≥3 conformance vectors per op including failure cases; zero contradictions with the §5.2 cast matrix; type-strict comparison rules explicit.

**F4 — Rule-op JSON + edge/evidence JSON.** Sources: §13, §14, App E conventions, App K. Output: JSON shape per rule op; full conversion-edge record; evidence record with field types. Completion check: App A's e1 and e2 conversions are expressible verbatim; fabricating rules (`default`,`const_set`) identifiable from the record alone (tier escalation is *computed* from rules, never stored as a flag someone could edit).

**F5 — `sense.v1` field types.** Sources: §22.1, §8.4, §22.3. Output: full field table — type, nullable?, refreshed-on-resume?, plus the reserved-subtree enforcement note (Planner rejects writes under `sense`). Completion check: every step of the §22.3 hydration order names only fields defined here.

**F6 — Registry entry JSON per class.** Sources: §10.1, §10.3, §10.4, App J. Output: concrete JSON for each class — compute / io (prepared + `aelio-db.query`) / tool / model (embedding App J ref) / registered-flow / dataset / template_layer. Completion check: every Call target in App A is expressible; boundedness declaration present and class-appropriate; effect_class present on all.

**F10 — Aelio DB DDL.** Sources: §24, App G, App I, App K. Output: table definitions in Aelio DB's schema language — incl. ledger (gapless `seq` per instance, hash-chain columns), unified `artifacts` + `artifact_history` (App K record), `continuations` (App I blob + pins as queryable columns for the §I.2 retention rule), CAS version columns on `states`/`flow_instances`. Completion check: every App G envelope/payload field has a storage answer; §I.2 retention is answerable by a single query; no table lacks `tenant_id` as leading key (§17.2).

**F11 — Conformance vector format + initial vectors.** Sources: §27, §8, App E, App G. Output: vector file format `{name, plan, initial_bag, injected_ledger?, expected: bag_hash | err.v1 | park_state}` + initial vectors: `Identity` as vector #1; every §8 op ≥2 vectors (happy + error), Park-capable ops + a park/resume vector; one replay-determinism meta-vector (run twice, compare `bag_hash`). Completion check: vectors are executable JSON, not prose; every §8 normative claim that is testable has a vector citing its section.

**F14 — Metrics schema.** Sources: §21, §18, App K. Output: table — metric name, type (counter/gauge/histogram), labels, aggregation window, and the *source event* (which ledger or artifact-history entry increments it). Completion check: both §21 kill criteria and §18 fallback rate are computable from defined metrics alone; no metric lacks a source event (unsourced metrics are unimplementable).


## Appendix G — Ledger entry schemas (F8, normative)

**Envelope (every entry):**
```json
{"seq": u64,            // per flow_instance, monotonic, gapless
 "turn_id": str, "nid": str|null, "kind": str,
 "ts": iso8601,          // informational; never replay-consumed; integrity-protected
 "payload": {…},         // Sol-shaped; canonical per §4.3; ≤1 MiB (§4.4)
 "payload_hash": blake3(canonical(payload)),
 "prev": blake3(canonical(previous entry incl. envelope))}   // per-instance hash chain (tamper-evident, §28)
```

**Replay categories:** `INJECT` = replay consumes payload instead of acting. `VERIFY` = replay recomputes deterministically and compares; mismatch ⇒ hard refuse (§12.3). Replay algorithm: walk the plan; at each ledger point pop next entry; kind/nid mismatch ⇒ refuse; VERIFY ⇒ recompute+compare; INJECT ⇒ write payload in.

| kind | payload | category |
|---|---|---|
| `turn_start` | `{trigger: message\|wake\|termination, channel, coalesced_count, input_refs[], flow_id, flow_rev, kernel_version, protocol_version}` | init |
| `sense_snapshot` | `{sense: SolBody}` | INJECT |
| `pathway_pick` | `{decision_point, scores[{pathway_id,score}], margin, entropy, picked, fallback_used, embedding_model}` | INJECT (pick); scores integrity-only |
| `nondet_value` | `{source: now\|uuid\|random, value}` | INJECT |
| `model_call` | `{target, template, model, prompt_hash, args_hash, output: SolBody, usage{in,out}, parse_ok, attempt}` | INJECT (output); **args_hash VERIFY** |
| `call_intent` *(write/external, §12.4)* | `{target, effect_class, args_hash, idem_key?, deadline_ms}` | **VERIFY** (args_hash) |
| `call_result` | `{target, outcome: ok\|err, output?: SolBody, err?: err.v1, duration_ms}` | INJECT |
| `read_result` *(read-class I/O incl. query AST)* | `{target, args_hash, output: SolBody}` | INJECT; args_hash VERIFY |
| `convert_fired` | `{edge_id, conversion, status_at_use, outcome: ok\|rule_fail, rule_index_failed?, input_hash}` | **VERIFY** (rules are pure — replay re-runs them) |
| `budget_trip` | `{scope_nid, meter, limit, observed}` | `ms` INJECT; `calls/tokens/size/iter` VERIFY |
| `timeout_trip` | `{scope_nid, ms_limit}` | INJECT |
| `guard_check` *(check=each only)* | `{guard_nid, at_nid, ok}` | VERIFY (pure predicate) |
| `skip_marker` | `{nid, path}` | VERIFY |
| `side_failed` / `finally_failed` | `{scope_nid, err: err.v1}` | INJECT |
| `park` | `{park_nid, until, event_key?, continuation_hash}` | VERIFY (continuation_hash recomputed) |
| `resume` | `{park_nid, wake{kind, payload: SolBody}, policy_recheck, guards_recheck[{nid,ok}]}` | INJECT |
| `turn_end` | `{outcome: completed\|parked\|erred, final_err?, bag_hash, persisted[]}` | **bag_hash VERIFY — the bit-identity check** |
| `termination_turn` | `{reason: gc\|kill, finallys_run[]}` | INJECT |
| `call_dispatch` *(F9 refinement)* | `{corr}` — appended when the frame actually leaves the server | VERIFY |
| `late_result` *(F9 refinement)* | `{corr, outcome, output?\|err?}` — result arriving after unknown-outcome was raised; triage feed, never re-enters the flow | informational |

**Boundary rule:** gate transitions and cold-path proposals are **not** turn-ledger entries — they live in artifact history (F13). The turn ledger records execution only.

**Refined recovery rule (F9):** `call_intent` **without** `call_dispatch` = provably-not-executed ⇒ safe `Tool.Transient` (any effect class). `call_intent` **+** `call_dispatch` without `call_result` = unknown outcome ⇒ `Internal`, fail-loud (§8.4). Unknown-outcome applies only post-dispatch.

**Durability ordering (write-ahead, from §12.4/§12.5):** `call_intent` durable **before** adapter dispatch; `turn_end` durable **before** the reply is released to the channel; batch-fsync permitted within a turn otherwise.

## Appendix H — Wire protocol (F9, normative)

**Encoding:** canonical-JSON frames (§4.3), length-prefixed, over WebSocket binary or gRPC byte stream — transport-agnostic at this layer. `protocol.major` bumps on breaking frame changes; server refuses unknown majors (§32).

### H.1 Handshake
```json
SDK→  {"frame":"hello","protocol":{"major":1,"minor":0},"sdk_version":str,
       "deployer_id":str,"auth":"<channel-token>","resume"?:{"last_corr_acked":str}}
SRV→  {"frame":"hello_ack","protocol":{...negotiated},"conn_id":str,
       "heartbeat_ms":int,"pending_tool_calls":int}
   |  {"frame":"hello_reject","reason":str,"supported_majors":[int]}
```

### H.2 Data-plane frames (reverse tool channel — the SDK's entire data surface, §35)
```json
SRV→  {"frame":"tool_call","corr":str,"target":"<id>@<v>","effect_class":str,
       "args": SolBody,                    // §4.2.4 projection — least privilege over the wire too
       "deadline_ms":int,"idem_key"?:str,"turn_ref":str}
SDK→  {"frame":"tool_result","corr":str,"outcome":"ok|err",
       "output"?: SolBody,"err"?: err.v1,"duration_ms":int}
both  {"frame":"ping","t":int} / {"frame":"pong","t":int}
SDK→  {"frame":"ack","corr":str}          // delivery acknowledgment
```

### H.3 Delivery & failure semantics (every rule maps to an existing lock)
- **At-least-once delivery:** server re-sends unresolved `tool_call` frames across reconnects until `tool_result` or deadline; **SDK dedups by `corr`** (in-flight ⇒ attach; completed ⇒ re-send cached result). `idem_key` forwarded so SDK-side adapters can enforce effect idempotency at their layer too (defense in depth on §8.4).
- **Deadline expiry:** authoritative server-side. Never-dispatched (no `call_dispatch` entry) ⇒ `Tool.Transient`, any class. Dispatched write/external ⇒ **`Internal` unknown-outcome** (§8.4 crash semantics over a socket) — *never* `Tool.Transient`, which is retryable and would double-fire effects. Dispatched read ⇒ `Tool.Transient`.
- **Duplicate `tool_result`:** server dedups by `corr`; duplicates acked + discarded.
- **Late result** (after unknown-outcome raised): `late_result` ledger entry; feeds triage; never re-enters the flow.
- **Disconnected at dispatch time:** fail fast `Tool.Transient` (nothing dispatched ⇒ provably safe); flows' `Fallback` handles per §31.
- **Heartbeat:** interval from `hello_ack`; N misses ⇒ connection marked down; tool reachability = connection state.

### H.4 Control plane (HTTP, deployer keys; scopes: push / approve / read_ledger / admin)
```
POST /v1/artifacts                  {class: tool|flow|imprint|template|dataset|pathway, body, expected_version?}
                                    → {id, version}        // push = new version (§32); pins/§25 automatic
GET  /v1/artifacts/{id}[@version]   ;  GET /v1/artifacts?class=…
POST /v1/artifacts/{id}/lifecycle   {action: approve|demote|retire}     // §16 reviewed-tier approvals live here
GET  /v1/instances/{id}/ledger      // audit read (§28)
GET  /v1/metrics                    // §21 schema
```
**Push-time Planner validation:** every flow/procedure push runs the full §29 Planner statically — broken flows are rejected **at deploy time**, never discovered at runtime. A push response includes the derived R/W sets and reach classification (§16.2) so the SDK can surface "this flow will require converter approval" to the deployer before anything runs.

## Appendix I — Continuation serialization (F12, normative)

**Format:** canonical JSON (§4.3), versioned envelope:
```json
{"cont_format": 1,
 "pins": {"kernel_version": str, "sol_version": str, "flow_id": str, "flow_rev": str},
   // Call-target & template pins live in the flow definition itself (§10.1) — pinning flow_id@rev suffices
 "identity": {"tenant": str, "flow_instance_id": str},
 "park": {"park_nid": str, "until": {...}, "event_key"?: str},
 "frames": [Frame, ...],          // root → parked node; see F.stack below
 "bag": {"imprint": "~<structural-hash>",   // Amendment #3
         "body": SolBody},
 "continuation_hash": blake3(canonical(all above))}
```
**Resume check order:** hash verify → kernel_version equal (else refuse-or-explicit-migrate, §25.3) → then §8.4 resume sequence (policy, Guards, sense refresh).

### I.1 Frame stack (per-scope dynamic state — what nids alone can't hold)
```json
Seq      {"nid", "step_index": int}
Map      {"nid", "element_index": int, "collected": [SolValue, ...]}     // §8.4: 0..k−1 held
Loop     {"nid", "iter_count": int}
Try      {"nid", "phase": "body" | {"handler": "<code-prefix>"} }        // caught err is at err_into in the bag
Fallback {"nid", "step_index": int, "prior_errs": [err.v1, ...]}
Guard    {"nid"}                                    // invariant re-evaluated on resume; no state
Budget   {"nid", "used": {"calls": int, "tokens": int, "ms": int}}
Timeout  {"nid", "ms_used": int}
Let      {"nid", "shadowed": {"<key>": SolValue | {"absent": true}, ...}}  // restore-at-exit set
Tee      {"nid", "phase": "body" | "side"}          // side parking is plan-time impossible (§8.4); frame exists for body-side parks under Tee.body
Once     {"nid", "idem_key": str}                   // resolved key, so resume dedups identically
```
Parking inside a `Try` *handler* is legal (handlers are ordinary instructions); the `phase` field is what makes resume-completion run `finally` correctly.

### I.2 Flow-revision retention (new rule surfaced by pinning)
A flow revision is **collectible only when no parked continuation and no promoted artifact pins it.** Resume always executes the pinned (old) revision even if newer revs exist — edit-safety for in-flight conversations, mechanically enforced. The control plane exposes pinned-rev counts so deployers see why an old revision persists.

## Appendix J — Prompt template format (F7, normative)

```json
{"template_format": 1,
 "id": "model.ask_phone",                       // version assigned by registry on push (§32)
 "model": {"id": "<provider-model-id>", "params": {"temperature": num, "max_tokens": int, ...}},  // pinned (§10.3)
 "output_imprint": "<declared-id>@<v>",          // §4.1.2c — the only validation authority
 "slots": {"<name>": {"type": fundamental-type, "sensitivity": "public|internal|pii|secret",
                       "required": bool}},
 "layers": [{"ref": "<layer-id>@<v>"} | {"inline": "<text>"}, ...],   // fixed order; refs are registered
                                                  // template_layer artifacts (tenant or vendor origin, §25.1 pins)
 "body": "<text with {{slot}} placeholders>",
 "exemplars": [{"slots": {...}, "output": {...}}, ...],
 "parse": {"mode": "json_imprint"}}               // single mode v0: extract JSON → validate → Model.Parse on fail, retry cap 1 (§11.3)
```

**Normative rules:**
1. **No template logic.** `{{slot}}` substitution is the entire template language — no conditionals, loops, or expressions. Conditional prompting = different templates selected by `Branch` in the flow; prompt decisions stay in the ledgered kernel.
2. **Slot rendering:** values render as canonical JSON literals (§4.3 forms — strings quoted+escaped). Unambiguous data boundaries; the model sees data as data; no delimiter ambiguity for injection to exploit. `pii` slots receive §15 shape-preserving masks unless the target's policy tags authorize raw (reviewed-tier decision, §10.3.5).
3. **Composition & hashing:** `composed_hash = blake3(canonical(resolved layers ∥ body ∥ exemplars ∥ model ∥ params))`; per-call `prompt_hash = blake3(composed_hash ∥ canonical(slot values))` — this is the App G `model_call.prompt_hash`, now exactly defined. Layer version bumps change `composed_hash` ⇒ §10.3.4 demotion cascade fires automatically.
4. **Push-time validation (extends App H):** slot names in `body` must all be declared; declared slots must all appear; **exemplar outputs must validate against `output_imprint`** — a template whose own examples don't parse is rejected at deploy.
5. Streaming: none in v0 (§35); completion-based ledgering either way.

## Appendix K — Artifact lifecycle state machine (F13, normative)

**States:** `proposed · rejected · shadow · canary · promoted · retired`. **Demotion is an event, not a state** — its target is `shadow` (or `canary` for kernel-bump demotions, §25.5). Applies to all artifact classes via §16.5; agreement predicate per class (converter: digest validation · pathway: retrospective outcome match · procedure: shadow-compare).

| From → To | Exact trigger |
|---|---|
| ∅ → `proposed` | Cold-path proposal emitted (single-flight, §15) |
| `proposed` → `rejected` | Structural check fails (§16.1.1); reason + rules_hash recorded; terminal — a *different* rules_hash starts fresh at `proposed`; same hash short-circuits to backoff (§13.1) |
| `proposed` → `shadow` | Structural check passes (automatic; shadow consumes nothing ⇒ approval-free in every tier) |
| `shadow` → `canary` | distinct_inputs ≥ 20 ∧ validation_rate ≥ 0.95 ∧ **(reviewed tier: deployer approval — REFINED: approval gates first *consumption*, i.e. here, not promotion)** ∧ (locked tier: unreachable — never auto-proposed) |
| `canary` → `promoted` | distinct canary inputs ≥ 20 ∧ downstream success ≥ 0.98 ∧ zero attributed `Guard.Violation` ∧ (auto tier: no unresolved semantic-review flag, §16.3) |
| `canary` → `shadow` *(demote)* | Attributed failure > 2% trailing ∨ any attributed `Guard.Violation` |
| `promoted` → `shadow` *(demote)* | Same as above ∨ imprint version bump (§4.1.8) ∨ flow edit (§4.1.6) ∨ template version bump where evidence-dependent (§10.3.4) ∨ manual (always available) |
| `promoted` → `canary` *(demote)* | Kernel version bump (§25.5 — canary-all) |
| any active → `retired` | Manual ∨ 3 demotions in window ∨ unused > window |
| `retired` → `shadow` | Manual revival only |

**Fast-track re-entry after demotion (micro-decision, conservative):** retained evidence persists, but re-`canary` requires a fresh shadow pass of ≥10 distinct inputs (half threshold) validating against the *current* digest/flow — fast, not free; a demotion cause must be re-tested against the world that caused it.

**Storage:** transitions append to artifact history (App G boundary rule) with `{ts, from, to, trigger, actor?: deployer|system, evidence_snapshot_hash}` — the audit answer to "who promoted this and on what evidence."

## Appendix D — Decision log
| Date | Section | Change | Rationale |
|---|---|---|---|
| 2026-07-26 | all | v0.1 scaffold created; statuses assigned | initial |
| 2026-07-26 | §4.1 | LOCKED: two-tier imprints; declared mandatory at persistence/Call/Model/Park boundaries; structural = canonical recursive hash; program-bearing bags barred from boundaries | A4.1 attack sequence: full-shape hashes unstable under unrelated keys → read-set is applicability authority; structural match carries no meaning → edge_id is application authority |
| 2026-07-26 | §4.1.6 | RATIFIED (user): strict edge-only warm reuse for v0; cross-edge matches = gate re-entry with pre-filled rules | canary is cheap, wrong promotion is not; revisit with §21 hint-frequency data |
| 2026-07-26 | A4.3, A13.1 | Closed as consequences of §4.1 | see above |
| 2026-07-26 | §4.2 | LOCKED + RATIFIED (user): edit-only bag model; single Call semantics `Call(id,args,into)` with mandatory `into`, root forbidden; Const sole whole-bag writer; literal instruction paths; atomic op writes; imprint-validated `into` commits | least-privilege callee input (G5, A17.2), static read/write sets (G2, waves, A6.2), fail-loud context preservation; Pipe sugar deferred to P2 |
| 2026-07-26 | §4.3, §4.4 | LOCKED + RATIFIED (user): canonical serialization (BLAKE3, sorted keys, int/float distinction, NaN/∞ forbidden) and v0 limits (depth 32, 1024 keys, 1 MiB, 10k lists, mandatory max_items on all/column scope) | hashing/replay/idempotency require canonical form; NaN/∞ have no agent-state use and poison determinism |
| 2026-07-26 | §4 | **Section fully LOCKED** — first section complete | foundations for §5, §6, §8, §12, §13 |
| 2026-07-26 | §5 | **Section fully LOCKED** + RATIFIED (user): cast matrix (TOTAL/CHECKED/MODE-REQUIRED/FORBIDDEN); `int→float` CHECKED with no lossy escape; checked arithmetic; imprint declaration remainder; validators-only refinements | cast = representation only; semantic mappings funneled to gate-audited rule ops |
| 2026-07-26 | §14 | RATIFIED (user): `trim` admitted to rule set under new admission rule (pure, total/fail-loud, meaning-free, locale-independent); case folding explicitly barred | closes dirty-scalar repairability gap without breaching the cast/map_enum wall |
| 2026-07-26 | §6 | **Section fully LOCKED** + RATIFIED (user): path grammar with quoted-bracket segments; plan-time R/W sets with prefix-aware Bernstein wave rule; Map/Filter imports-only privilege unification; trace-visible skip; list home killed (synopsis Lock #1 resolved) | one privilege model at every boundary; wave-readiness without spec change |
| 2026-07-26 | §11 | **Section fully LOCKED** + RATIFIED (user): closed taxonomy (+Budget.Iter, +Guard.Violation); prefix catch matching; normative retryability; err.v1 Sol shape with cause chain; kernel never auto-retries | determinism, honest budgets, honest failure metrics |
| 2026-07-26 | §4.1.6 | **AMENDMENT #1** (RATIFIED, user): edge_id = (tenant, flow_id, producer_nid, consumer_nid) — nid-based, version-free; flow edit demotes all flow edges to shadow with fast-track re-promotion | versioned edge_id contradicted edit-survivability (any edit orphaned all edges); nids give stable identity; brutal-conservative demotion fits strict-edge posture, replaceable by dataflow impact analysis in v2 |
| 2026-07-26 | §8.1–8.2 | Batch 1 LOCKED + RATIFIED (user): nid node identity; Const/Identity/Seq/Let/Branch/Tee entries; Seq is not a transaction — staging idiom instead of Atomic op; single predicate concept; Tee plan-time dataflow isolation | A8.5 resolved |
| 2026-07-26 | §8.3 | Batch 2 LOCKED + RATIFIED (user): Loop raises on exhaustion; Map fail-fast atomic; Try err_into + original-error-wins finally; Fallback step-originated-only advance; Guard check modes (A8.6 resolved) | silent caps hide bugs; kernel never accepts partial data; root causes survive finally; budget exhaustion ≠ failed alternative |
| 2026-07-26 | §8.4, §8 | **§8 fully LOCKED** + RATIFIED (user): active-time-only ms meters; Once at-most-once with fail-loud unknown outcome; acyclic call graph → termination theorem (G1 proof sketch); Park matrix; reserved runtime-owned `sense` subtree; abandoned-park termination turns | A8.1–A8.6 all resolved; §23 core inherited; A22.3 bug class designed out at language level |
| 2026-07-26 | §12 | **Section fully LOCKED** + RATIFIED (user): depth-first sequential order; ledger kind enumeration; replay = pure function with hard-refuse divergence; universal intent/result accounting for write/external Calls; ledger-as-WAL. **PART III COMPLETE — deterministic substrate fully specified.** | replay honesty; forgotten-Once no longer causes silent double-execution |
| 2026-07-26 | §13, §15 | **Both LOCKED** + RATIFIED (user): lifecycle with `rejected` state (anti-proposal-loop); distinct-input evidence; sensitivity levels with shape-preserving masking; single-flight cold path; containment theorem | evidence can't be inflated; injection downgraded to the gated failure class |
| 2026-07-26 | §16, §17 | **Both LOCKED** + RATIFIED (user) after decision-procedure run: shadow redefined as validate-without-consume (bootstrap solved); **reach-based reviewed tier** (amendment — original feeds-a-write rule missed the founding bug, which feeds a predicate); advisory-only LLM judge; v0 constants; generic parametrized gate; **mutation harness with published catch rates** committed P1. Tenant isolation absolute; vendor origin class. | walkthrough 1 broke the original tier rule; precedent = progressive delivery; falsifiability = the open-source credibility story |
| 2026-07-26 | §14 | **LOCKED**: per-rule entries; sequential rule execution; map_enum RuleFail semantics; **fabrication escalation** (default/const_set ⇒ reviewed tier regardless of reach); non-computation by construction. **PART IV COMPLETE.** | fabricated data is inherently semantic |
| 2026-07-26 | §18–§21 | **PART V COMPLETE** + RATIFIED (user): classifier hygiene with mandatory fallback + cap 8; pinned embedding model (Lighthouse bug class impossible); promotion=registration for procedures; mining ≥5/30d; dumb-honest conservative attribution with documented biases; pre-registered kill criteria (60% converters/90d; 10% procedures/6mo) | biases err toward cheap demotion, never expensive promotion; falsifiability over pitch |
| 2026-07-26 | §10 | **LOCKED** incl. new §10.3 prompt discipline (user-raised gap): prompts are registered versioned artifacts; args-only slot filling; deterministic composition with ledgered prompt_hash; template version bump demotes dependent learned artifacts; slot-level sensitivity masking | "correct prompt always goes in" becomes structural, not aspirational |
| 2026-07-26 | §22–§28 | **PART VI COMPLETE** + RATIFIED (user): single-flow + read-only detours (no stack v0); normative hydration order; token-based wake routing; unified artifacts table + CAS; 5-point migration doctrine (kernel bump ⇒ canary-all); v1-shipping tooling; §8 entries as conformance vectors; §28 as traceability page | ordering bug made law; deferral over stack complexity |
| 2026-07-26 | §10.4, §29 | RATIFIED (user): query discipline — closed query AST + registered I/O targets, mandatory limits, no query text anywhere; interpreter formalized as Planner + Executor two-phase machine | user-raised: prompt factory + interpreter + seamless Aelio DB query construction — all three now structural |
| 2026-07-26 | §1–§3, §9, §30, App A–B | Closing pass: positioning, escape-hatch table with traced guarantees + honest costs, threat matrix; Compute catalog v0; milestones refreshed; login flow worked example; traceability matrix filled. **ALL 30 SECTIONS LOCKED.** | every G-guarantee cites a locked mechanism; doc is implementable end to end |
| 2026-07-26 | Part VIII (§31–§35) | RATIFIED (user, all five): SDK reverse-channel tool execution (server never holds customer keys); control/data plane split with versioned pushes + versioned wire protocol; per-instance actor + debounce queue; tenant-shard scale-out; server-side BYO-key provider trait; server-side channel adapters; channel-scoped identity; non-streaming v0. License: Apache-2.0 (RATIFIED via delegation). Aelio DB committed as the store; `aelio-store` keeps trait boundary for test doubles only. | reverse channel reuses Park/token/ledger locks verbatim; sweep additions: wire versioning, double-text debounce, identity posture, streaming posture |
| 2026-07-26 | §33, App E, App F | RATIFIED (user): configurable coalescence (debounce_ms/max_coalesce/queue_depth, coalesced input as ordered list Sol). Elaboration phase opened: Appendix E instruction schema delivered (F1); F2–F14 backlog registered | doc transitions from decision record to build reference; elaboration ≠ ratification |
| 2026-07-26 | App G (F8) | Elaborated; 3 micro-decisions defaulted (user offered veto): per-instance hash chain; INJECT/VERIFY replay classification with args_hash verification; turn-ledger/artifact-history boundary; durability ordering (intent-before-dispatch, turn_end-before-reply) | replay becomes a proof of the deterministic prefix, not just a re-run |
| 2026-07-26 | App H (F9) + App G refinement | Elaborated; **spec refinement caught**: deadline expiry on dispatched write/external ⇒ Internal unknown-outcome, never retryable Transient (double-effect hole closed); new `call_dispatch`/`late_result` kinds; at-least-once + corr dedup; push-time Planner validation with reach pre-classification | Transient-on-dispatched-write would have double-fired effects via Fallback; deploy-time rejection beats runtime discovery |
| 2026-07-26 | App I (F12) + §4.1.2(d) | **AMENDMENT #3** (defaulted, veto offered): park snapshots use structural imprints — park/resume crosses no semantic boundary; declared imprints stay mandatory at real boundaries. Frame-stack schema (incl. Try handler phase, Let shadow-save, Map collected); flow-rev retention rule (collectible only when unpinned) | declared-imprint-on-park bought friction, not safety; in-flight conversations are edit-safe mechanically |
| 2026-07-26 | App J (F7) | Elaborated; micro-decisions defaulted: **no template logic** (conditional prompting = Branch-selected templates); JSON-literal slot rendering; single json_imprint parse mode; exemplar push-time validation; exact prompt_hash definition | template logic would fork prompt behavior outside the ledger's sight |
| 2026-07-26 | App K (F13) + §16 refinement | **SPEC REFINEMENT**: reviewed-tier approval moved to shadow→canary (before first consumption) — approval-at-promotion left a 50-use consumption window on write-reaching edges, the exact harm the tier exists to close. Demotion clarified as event (target: shadow, or canary for §25.5). Conservative fast-track re-entry (half-threshold fresh shadow). Transition audit records | canary consumes; consumption before review was the active≡loggedin window reopened |
| 2026-08-01 | §10.4, App E, App F | **AMENDMENT #4 (RATIFIED, recommended convergence profile):** Prism's implemented closed multimodal envelope replaces the former single-op flexible-read AST; legacy `QueryAst` is compile-time sugar. Mandatory projection/limit, explicit RRF fusion, flat scalar predicates, physical schema validation, graph budgets, and replay-injected read results are normative. All derived annexes marked complete. | one stored/executed query language eliminates semantic drift; implementation and adversarial tests already enforce the stronger bounded envelope; free query text remains impossible |
