# Aelio L0 — Normative Agnostic Substrate Glossary

**Status:** normative living contract (v0.1)  
**Kernel v0:** **ADOPTED** (see §4 and [`AELIO_L0_KERNEL_COMPOSITION_LAB.md`](./AELIO_L0_KERNEL_COMPOSITION_LAB.md))  
**Supersedes for L0 decisions:** rows in `aelio_dsl_dictionary.md` §§0–3 where this doc conflicts  
**Companions:** `AELIO_L0A_COMBINATORS_GUIDE.md`, Rust `Sunjet/Astrolobe/crates/aelio/src/ops/`  
**Out of scope here:** L1 ability semantics (except the buildability gap check in §5)

---

## How to read this document

| Field | Meaning |
|---|---|
| **Decision** | `keep` · `keep-as-sugar` · `defer` · `demote-to-L1` · `demote-to-registry` · `drop` |
| **Status** | `normative` (must exist) · `aspirational` (allowed later, not required for L1 spine) · `implemented` · `partial` |
| **Agnostic?** | Passes the four tests below |

### Agnosticism tests (every L0 op)

1. Explainable without any product-domain noun (no phone, CRM, WhatsApp, OTP, …).
2. Sees only `Value` / nested `Op` / budgets / `ReasonCode` / effect env — not tenant schemas.
3. Same op serves a clinic, CRM, and game bot unchanged.
4. Fail → demote to L1 ability, repair registry, or tenant tool — **not** L0.

### Universal L0 rules (locked)

1. **Totality.** Every evaluation returns `Result` with a closed `ReasonCode`. No throws. No overloaded null.
2. **No silent coercion.** Numeric/string conversions are explicit pure ops.
3. **Bounded everything.** Loops, maps, retries, fan-out carry a declared ceiling.
4. **Closed combinator set.** Tenants cannot invent L0-A or L0-C primitives.
5. **`Call` is the only bridge** from an Op tree into named L0-B pure ids and L1 abilities.
6. **Domain never enters L0.** Domain nouns live in L1 contracts, tenant declarations, or repair registries invoked *by name* via `Call`.

---

## 0. Eval contract (Session 0) — LOCKED

### 0.1 Core terms

| Term | Definition | Decision | Status |
|---|---|---|---|
| **`Value`** | Dynamic JSON-shaped tree: `Null \| Bool \| Int \| Float \| Str \| List \| Map`. Carrier of all data through eval. Domain-agnostic by construction. | keep | normative / implemented |
| **`Op`** | One executable node in a composition tree (combinator, leaf, or `Call`). | keep | normative / implemented |
| **`eval(op, input) → Result`** | Run an Op on a `Value`; produce `Value` or typed error; may set `Suspended` via `Park`. | keep | normative / implemented |
| **`ReasonCode`** | Closed failure taxonomy (`Validation`, `BudgetExceeded`, `NeedsRepair`, …). Catch keys in `Try` are these codes, not exception classes. | keep | normative / implemented |
| **`AelioResult<T>`** | `Result<T, AelioError>` where error carries `ReasonCode` + message. | keep | normative / implemented |
| **`Call{id, args}`** | Invoke a **named** pure op or L1 ability by id. Spends call budget. The Op tree stays agnostic; the *registry* binds ids. | keep | normative / implemented |
| **`EvalCtx`** | Evaluator state: invoke fn, once-seen, budgets, locals, suspended flag. Not tenant-specific. | keep | normative / implemented |

### 0.2 Type universe (agnostic carriers)

| Type | Role | Decision | Notes |
|---|---|---|---|
| `Int`, `Float`, `Bool`, `Str`, `Bytes` | Scalar carriers | keep | normative |
| `Decimal` | Fixed-point numeric | keep | aspirational in Rust Value today (use Int/Float carefully) |
| `List`, `Map`, `Option`, `Result` | Structure | keep | normative |
| `Instant`, `Duration`, `Interval`, `Tz` | Time carriers (values, not `Now`) | keep | normative; calendar ops take explicit `Tz` |
| `Vector` | Dense f32 embedding carrier | keep | normative; production of vectors is L1/`Call`, not L0-B |
| `Id` | Opaque stable identifier string | keep | normative |
| `Path` | Minimal structure path (`a.b`, `a[0]`, `a[*]`) | keep | normative |
| `Pattern` | Pre-validated linear-time regex | keep | normative |
| `TypeTag` | Runtime discriminant | keep | normative |

### 0.3 Call-bridge rules (locked)

```text
Op tree  --Call{id}-->  invoke(id, args, input)
                           │
                           ├─ id in L0-B pure table  → deterministic free glue
                           ├─ id in L1 ability table → Sense / Understand / …
                           └─ unknown id             → Err(NotFound | Unsatisfiable)
```

- L0 never hardcodes ability behavior inside combinators.
- L0-B functions may be exposed *as* Call ids; they remain pure and agnostic.
- Personality, tools, CRM fields never appear in `Op` enum variants.

---

## 1. L0-A — Combinators (Session 1)

Closed set. Branching factor of tier-2 compositional search. All take/return `Value` via child Ops — **I/O agnostic**.

### 1.1 Plumbing leaves

| Op | Contract | Decision | Status | Agnostic? |
|---|---|---|---|---|
| **`Const(v)`** | Ignore input; return fixed `Value` `v`. | keep | normative / implemented | yes |
| **`Identity`** | Return input unchanged. | keep | normative / implemented | yes |
| **`Call{id, args}`** | Named work bridge (see §0.3). | keep | normative / implemented | yes (id is opaque) |
| **`Let{bindings, body}`** | Bind locals from Ops on original input; eval body; scope ends. | keep | normative / implemented | yes |
| **`Tee{body, side}`** | Eval body; eval side on body output for effect; return body result. | keep | normative / implemented | yes |
| **`Pipe`** | Sugar for `Seq`. | keep-as-sugar | aspirational (dictionary) | yes |

### 1.2 Control shape

| Op | Contract | Decision | Status | Agnostic? |
|---|---|---|---|---|
| **`Seq[A,B,…]`** | Thread output→input left-to-right; stop on Err or Park. | keep | normative / implemented | yes |
| **`Branch{pred, then, else}`** | Pred → Bool; arms see **original** input, not the bool. | keep | normative / implemented | yes |
| **`Loop{while, body, max_iter}`** | Bounded while; exceed → `LoopBudgetExceeded`. | keep | normative / implemented | yes |
| **`Fallback[A,B,…]]`** | Same input to each; first Ok wins. | keep | normative / implemented | yes |
| **`Try{body, catch, finally?}`** | On Err, dispatch by `ReasonCode`; unmatched bubbles. | keep | normative / implemented | yes |
| **`Guard{invariant, body, on_violation}`** | Fail-closed precondition; false → Err(code). | keep | normative / implemented | yes |
| **`Budget{body, calls?, tokens?, ms?}`** | Nested budgets **intersect (min), never widen**. | keep | normative / partial (`ms` weak) | yes |
| **`Timeout{body, ms}`** | Wall-clock deadline → `Timeout`. | keep | normative / partial (structural stub) | yes |
| **`Once{body, idem_key}`** | Run effectful body at most once per key; replay-safe. | keep | normative / implemented | yes |
| **`Park{until}`** | End turn; wait Event / Ttl / Instant. **Not Sleep.** | keep | normative / implemented | yes |
| **`Par{…, merge, max_concurrency}`** | Parallel ops + mandatory merge rule. | defer | aspirational | yes |
| **`Switch{on, cases, default}`** | Multi-way on closed labels. | defer | aspirational (sugar over Branch) | yes |
| **`ForEach{list, max_items, mode}`** | Per-item body serial/parallel. | defer | aspirational (overlaps Map) | yes |
| **`Retry{body, policy}`** | Re-run idempotent bodies only. | defer | aspirational | yes |
| **`Race{…}`** | First success; pure/read-only only. | defer | aspirational | yes |
| **`Memo{body, key, ttl}`** | Cache pure results. | defer | aspirational | yes |
| **`Debounce{body, key, window}`** | Suppress repeated outbound work. | defer | aspirational (proactive) | yes |

### 1.3 Higher-order collections (combinators)

| Op | Contract | Decision | Status | Agnostic? |
|---|---|---|---|---|
| **`Map{body, max_items}`** | Transform each list item; bound mandatory. | keep | normative / implemented | yes |
| **`Filter{pred, max_items}`** | Keep items where pred is true. | keep | normative / implemented | yes |
| **`Reduce` / `FlatMap` / `Any` / `All` / `Find` / `Partition` / `SortBy` / `GroupByOp`** | Higher-order list folds. | defer | aspirational (macros over Map/Filter + pure) | yes |

### 1.4 L0-A decisions summary

- **Normative core (must ship):** Const, Identity, Call, Seq, Branch, Loop, Try, Guard, Fallback, Tee, Once, Budget, Timeout, Map, Filter, Let, Park.
- **Aspirational (do not block L1 spine):** Par, Switch, ForEach, Retry, Race, Memo, Debounce, extra collection combinators, Pipe sugar.
- **No domain combinators** ever (no `AskOtp`, no `SendWhatsApp`).

---

## 2. L0-B — Pure operations (Session 2)

Total, deterministic, side-effect-free, cost class free. Large and boring on purpose.

### 2.1 Keep as normative L0-B (agnostic)

| Category | Ops (representative) | Status |
|---|---|---|
| **Numeric** | Add, Subtract, Multiply, Divide, Modulo, Abs, Negate, Sign, Round, Floor, Ceil, Min, Max, Sum, Avg, Clamp, Compare, InRange, … | normative; **partial** in Rust |
| **Type conversion** | ToInt, ToFloat, ToBool, ToStr, TypeOf, IsType, Cast, … | normative; partial |
| **String** | Length, Strip, Lower, Upper, Slice, Contains, StartsWith, EndsWith, Replace, Concat, Join, SplitBy*, Regex*, IsBlank, EqualsIgnoreCase, … | normative; partial |
| **Structure / Path** | GetPath, GetPathAll, SetPath, HasPath, Keys, Values, Pick, Omit, MergeShallow, ParseJson, ToJson, … | normative; partial |
| **Collections (data)** | First, Last, Take, Count, Append, Concat, Reverse, … (no Op argument) | normative; partial |
| **Time (pure)** | DateAdd/Sub/Diff, Format, ParseDate, Interval*, Duration*, ConvertTz — all with explicit `Tz` | normative; thin in Rust |
| **Logic** | And, Or, Not, Equals, DeepEquals, Coalesce, IfElseValue | normative; partial |
| **Validation (generic)** | ValidateType, ValidateRange, ValidateLength, ValidateEnum, ValidatePattern, ValidateRequired, ValidateSchema | normative; partial |
| **Encoding / hash** | Hash, Base64*, Url*, Hex*, IdemKey, Redact(paths) | normative; partial |
| **Text metrics (non-semantic)** | WordCount, CharClassProfile, IsAllDigits, IsAlphanumeric, LongestCommonPrefix | normative; partial |

### 2.2 Demote / quarantine (fail agnosticism or belong elsewhere)

| Op / group | Decision | Rationale |
|---|---|---|
| **`NormalizePhone`** | **demote-to-registry** | Region/E164 is domain/locale policy. Expose as named repair id (`repair.normalize_phone`) callable via `Call` / `Bind.Normalize`, not a privileged L0-B primitive in the *normative* closed mindshare. May remain as an optional registered pure function. |
| **`NormalizeEmail` / `NormalizeUrl`** | **demote-to-registry** | Same: format conventions are product policy. |
| **`ValidateFormat` variants `Email\|E164\|Url\|Uuid\|Iso8601\|Ipv4\|CreditCard`** | **keep skeleton, demote catalogs** | Keep `ValidateFormat{value, format_id}` where `format_id` is a **registered** format validator. Do not hardcode CreditCard/E164 as eternal L0 nouns in the glossary core — register them. |
| **`HumanizeDuration`** | **demote-to-L1** or registry | Locale prose (“3 days ago”) is presentation; Express-adjacent. |
| **`SplitSentences`** | **demote-to-L1** | Linguistic; overlaps Understand.Tokenize. Keep SplitByDelimiter/Whitespace/Regex/Length/Lines in L0-B. |
| **`Slugify` / `RemoveDiacritics`** | **defer** | Borderline; useful glue but locale-policy flavored. Aspirational L0-B or registry. |
| **`TokenCountApprox{model_family}`** | **demote-to-L1 / provider** | Model-family coupling is not substrate-pure. |
| **`skip_split_clauses`** | **demote-to-L1** | Helper for Understand.SplitClauses; must not be treated as general L0 vocabulary. |
| **`Sample{list,n,seed}`** | **keep** (pure if seeded) | Agnostic; seed required for determinism. If unseeded → L0-C Random. |

### 2.3 L0-B rules locked

1. Pure ops never read clocks, RNG, network, or durable stores.
2. No embedding / LLM inside L0-B (`Vector` may be a Value; creating it is a `Call`).
3. Overflow → error, never wrap.
4. Regex engine must be linear-time; patterns validated at registration.

---

## 3. L0-C — Effects (Session 3)

Not pure. **Every call is ledger-visible** (recorded for replay). Domain-agnostic: they touch time, identity, randomness, scheduling, and audit — never CRM.

| Op | Contract | Decision | Status | Agnostic? |
|---|---|---|---|---|
| **`Now`** | → Instant; recorded | keep | normative / implemented | yes |
| **`Uuid`** | → Id; recorded | keep | normative / implemented | yes |
| **`Random{seed?}`** | → Float; seeded and/or ledgered | keep | normative / implemented | yes |
| **`Park{until}`** | Suspend eval / end turn (also an L0-A leaf) | keep | normative / implemented | yes |
| **`LedgerAppend{kind, payload}`** | Append audit record → receipt | keep | normative / implemented | yes |
| **`Emit{event}`** | Internal bus event | keep | normative / aspirational | yes |
| **`Schedule{at, op_ref, idem_key}`** | Future work | keep | normative / aspirational | yes |
| **`Cancel{schedule_id}`** | Cancel scheduled work | keep | normative / aspirational | yes |
| **`MetricInc` / `MetricObserve`** | Telemetry | keep | normative / partial | yes |
| **`TraceSpan{name, body}`** | Structured trace wrapper | keep | aspirational | yes |
| **`Sleep`** | — | **drop** | banned forever | — |

### 3.1 Effect rules locked

1. No `Sleep`. Waiting is always `Park`.
2. Nondeterminism (`Now`, `Uuid`, `Random`) must be replay-recorded or seeded.
3. `LedgerAppend` payloads are `Value` — schema of *kind* strings is convention above L0, not L0 variants.
4. Scheduling references `op_ref` / idempotency keys as opaque strings.

---

## 4. Kernel v0 — ADOPTED (finalize + build against this)

The **minimal agnostic foundation** that can run Op trees. Domain services are L1+ later.

### 4.1 Kernel v0 — L0-A

```text
Const  Identity  Call
Seq  Branch  Loop  Try  Fallback  Guard
Budget  Timeout  Once  Park
Let  Tee  Map  Filter
```

### 4.2 Kernel v0 — L0-C

```text
Now  Uuid  Random  Park  LedgerAppend
```

(`Schedule` / `Cancel` / `Emit` remain normative platform effects but are **Wave-2 kernel**, not Kernel v0 build blockers.)

### 4.3 Kernel v0 — L0-B categories

```text
numeric basics · string basics · path get/set · list first/take/count
logic · generic validate · hash / idem_key
```

Repair/format catalogs (`NormalizePhone`, `E164`, …) = **registered Call targets**, not kernel nouns.

### 4.4 Full normative card (broader than Kernel v0)

### L0-A normative (includes deferred aspirational elsewhere)

Same as §4.1 for the closed *required* set; aspirational `Par`/`Retry`/… stay outside Kernel v0.

### L0-C normative (full platform)

```text
Now  Uuid  Random  Park  LedgerAppend
Schedule  Cancel  Emit  MetricInc  MetricObserve
```

### L0-B normative (categories, not every name)

```text
numeric · convert · string · regex · path/structure · list-data
pure-time · logic · generic-validate · hash/encode · text-metrics
```

---

## 5. L1 buildability gap check (Session 4)

Question per family: *Can this be an L0 tree of Calls into this ability, without new domain combinators?*

| L1 family | Buildable on frozen L0? | Notes / gaps |
|---|---|---|
| **Sense** | yes | `Call("Sense.*")`; snapshot assembly is runtime, not new L0. |
| **Understand** | yes | `Call` + Branch/Fallback/Budget for S⇄L escalation. SplitClauses pre-gate is L1 helper, not L0. |
| **Recall** | yes | `Call("Recall.*")`; Fuse can be L0-B pure over hit lists **or** Call. Shapes = L2 blocks over L0+Call. |
| **Judge** | yes | Pure Judges as Call or L0-B; Verify/Groundedness as L Calls. |
| **Bind** | yes | Resolve/Validate as Call; repairs via registered normalize ids. |
| **Invoke** | yes | `Once` + `Tee(LedgerAppend)` + `Call("Invoke.Call")` + Sig Calls. |
| **Sig** | yes | Pure shape/hash/match as Call or L0-B (`Shape`/`SigHash`). |
| **Express** | yes | Templates/Ask/Synthesize as Call; Sequence with Park for confirm loops. |
| **Remember** | yes | Effectful Calls; no new L0 write primitive required beyond LedgerAppend for audit. |
| **State / Policy** | yes | Calls; Policy must remain **executor invariant** wrapping Invoke (not only composed in). |
| **Registry** | yes | Discovery Calls; not L0. |
| **Learn** | yes | SituationKey/LookupTier/Compose/ProposePath as Calls; paths are Op trees. Compose search uses L0-A closed set as branching factor — **why L0-A must stay small**. |
| **Proactive** | yes | Schedule/Emit/Park + Calls; Debounce aspirational L0-A helps later. |
| **Observe** | yes | Trace/Metrics Calls + LedgerAppend. |

### Missing L0 primitives? (gap verdict)

| Candidate gap | Verdict |
|---|---|
| Domain-specific combinators | **Rejected** — use L1/L2. |
| Stronger `Timeout` / Budget `ms` | **Needed for production** — already normative; implement fully (status→implemented). |
| `Par` / `Retry` | Useful; **aspirational**, not blocking Sense→Invoke spine. |
| First-class `Vector` ops in Value | Optional; embeddings stay behind Call. |
| Durable `Once` outside in-memory set | Runtime/storage concern, not a new Op variant. |

**Conclusion:** Frozen L0 is **sufficient** to host the L1 surface as named Calls inside L0-A trees. Remaining work is implementing partial ops and L1/L2 bodies — not inventing domain L0.

---

## 6. Implementation status snapshot (Rust)

| Layer | Implemented today (representative) | Gaps |
|---|---|---|
| L0-A | Const, Identity, Call, Seq, Branch, Loop, Try, Guard, Fallback, Tee, Once, Budget, Timeout*, Map, Filter, Let, Park | Timeout/Budget ms enforcement; aspirational combinators absent |
| L0-B | Add/Sub/Mul/Div, clamp, compare, many string/path/list/validate/normalize/hash helpers | Large dictionary surface still missing; Decimal; rich time |
| L0-C | Now, Uuid, Random, LedgerAppend (+ Park in A) | Schedule, Cancel, Emit, TraceSpan thin/absent |

\*Timeout present but does not fully enforce wall clock in sync eval.

---

## 7. Change control

1. New L0-A/C op requires: agnosticism tests, impact on tier-2 search branching, decision log entry.
2. New L0-B op requires: purity + determinism proof; no domain nouns in the name unless registered repair id.
3. Demotions in §2.2 are **glossary-normative**; code may keep helpers until Call/registry wiring lands — do not expand domain L0 further.
4. This file is the L0 contract for open-source / production design participation.

---

## 8. One-sentence essence

**L0 is a closed, domain-blind instruction set: combinators shape evaluation, pure ops reshape Values, effects touch time/ledger/schedule — and `Call` is the only door into L1 meaning.**
