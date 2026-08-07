> **Reference-design notice (2026-08-07).** This document is an **external reference design**,
> not the specification for this repository's actual system. The real, authoritative spec is
> [`AELIO_DSL_MOTHER.md`](../claude_context/AELIO_DSL_MOTHER.md) (repo-root symlink), implemented
> under `aelio-os/`. This document's terminology (`starlark-rust`, `Astrolobe`, `redb`,
> `harness-core`) does not appear anywhere in the actual codebase; it describes a parallel design
> for the same class of problem, not a diff against real code. See `FLAGS.md` entry **F-032** for
> the full terminology map to the real system (`aelio-sol` / `aelio-kernel` / `aelio-agent` /
> `aelio-db-*` / Sol contracts) and the resolution of which gaps this doc surfaced were worth
> acting on versus which are a different-but-valid design choice already made elsewhere.

# Harness Kernel — Specification v4

**Consolidated.** Supersedes v3, v3.1, v3.2, and the convergence plan's findings. Amendment history in Appendix A.

**Kernel:** Rust, Starlark (`starlark-rust`) over a closed native op catalog
**Control-plane store:** Astrolobe (vector + graph + full-text + columnar)
**Data-plane store:** `redb`, embedded, no external dependency
**Thesis:** the product is **reuse under proof** — recognising that a new request instantiates an abstract workflow already discovered, and running it deterministically with pre-execution authorisation.

---

# PART I — FOUNDATIONS

## 1.1 The five invariants

Every rule in this document preserves one of these. A feature violating one is wrong.

**I1 — Determinism.** Identical inputs plus identical effect journal produce byte-identical output. Verified continuously by replay, never assumed.

**I2 — Pre-execution authorisation.** The complete effect set a program can reach is computable before the first instruction and is checked against the invoking principal's grants at that moment.

**I3 — Precision over recall.** A missed match costs one authoring pass. A false match costs correctness, silently, at scale. Ambiguity resolves toward rejection.

**I4 — Auditability.** Every served answer traces to a skeleton version, hole bindings, join paths, an effect journal, and a hash-chained ledger entry.

**I5 — Trust is content-addressed.** Any value a security decision depends on is keyed by content hash and verified on read. Derived indexes accelerate lookup; they never constitute authority.

## 1.2 Terminology

| Term | Definition |
|---|---|
| **Op** | Native Rust primitive. Closed catalog. Total, pure, cheap. |
| **Harness** | Deterministic Starlark program composing ops and other harnesses. |
| **Conductor** | Stochastic supervisor above the determinism boundary. Not a harness. |
| **Skeleton** | Normalised harness AST with literals and field references abstracted into typed holes. |
| **Signature** | BLAKE3 of a canonical skeleton. Structural reuse key. |
| **Binding** | Assignment of values to holes, including resolved join paths and temporal resolutions. |
| **Plan** | Skeleton + binding. The executable unit. |
| **Turn** | One user message and its response. Identified by a client-supplied `turn_key`. |
| **Trace** | Ordered, hashed record of one execution. |
| **Journal** | Recorded effect results within a trace, enabling replay. |
| **Principal** | Identity on whose behalf execution occurs. |
| **Contract** | A tool's type signature plus semantic properties. |
| **Schema graph** | Per-tenant graph of entities, fields, relationships, tool bindings. |

## 1.3 Layer model

```
┌────────────────────────────────────────────────────────────────┐
│ L4  CONDUCTOR       triage · extraction · verification · render │
├════════════════ DETERMINISM BOUNDARY ═════════════════════════┤
│ L3  HARNESSES       Starlark, composed, replayable              │
│ L2  OPS             closed native catalog, Rust                 │
│ L1  EFFECT DRIVER   permission · taint · journal · retry        │
│ L0  TOOLS           customer functions, contract-bound          │
└────────────────────────────────────────────────────────────────┘
```

The closed catalog lives at L2. Starlark is the composition glue above it, not a replacement.

## 1.4 Why the conductor is not a harness

A harness is defined by replayability. The conductor makes model calls, which are stochastic. If the conductor is a harness, harnesses may make model calls freely, and nothing below is replayable.

Nondeterminism inside harnesses remains permitted — model calls, clock reads — but only through the effect system, journaled, with replay reading the journal instead of re-invoking.

---

# PART II — NUMERIC, TEMPORAL, AND ORDERING FOUNDATIONS

Everything I1 depends on. Get this wrong and every guarantee above it is decorative.

## 2.1 Numeric tower

Three types, **no implicit coercion**.

| Type | Representation | Use |
|---|---|---|
| `Int` | arbitrary precision (Starlark native) | counts, IDs, indices |
| `Float` | IEEE-754 binary64 | measurements, ratios, statistics |
| `Decimal{scale}` | 128-bit fixed point | **money, and anything reconciled against another system** |

- `add(Int, Float)` is a type error. Conversions are explicit: `to_float`, `to_decimal(scale)`, `to_int(rounding_mode)`.
- `Decimal` arithmetic uses banker's rounding; the mode is declared on the op, never inferred.
- `div` on `Int` returns `Decimal`. Integer division is `div_floor`.
- A tool declaring a `Decimal` field and returning a JSON float is a **contract violation**, failed at the effect boundary, never silently coerced.

## 2.2 Aggregate return types

| Op | `Int` in | `Float` in | `Decimal{s}` in |
|---|---|---|---|
| `sum` | `Int` (exact) | `Float` | `Decimal{s}` (exact) |
| `min` / `max` | `Int` | `Float` | `Decimal{s}` |
| `count_rows` / `count_values` | `Int` | `Int` | `Int` |
| `mean` | **`Decimal{6}`** | `Float` | **`Decimal{s+6}`** |
| `median` | `Int` odd / **`Decimal{6}`** even | `Float` | `Decimal{s}` odd / `Decimal{s+6}` even |
| `stddev` | `Float` | `Float` | `Float` |

`mean` over `Int` returns `Decimal`, not `Float` — averaging order counts and getting a binary float back is how rounding error enters a system that had none. `stddev` returns `Float` regardless because a square root has no exact decimal form, and the type should say so rather than implying exactness.

Signature families split by input type: `agg_scalar_int`, `agg_scalar_float`, `agg_scalar_decimal`. This costs hit rate and is worth it. A skeleton averaging a money column and a temperature column with identical machinery will eventually round someone's invoice.

## 2.3 Null semantics

Null handling is **structure, never a parameter**, and explicit in the op name.

| Op | `[1, null, 3]` |
|---|---|
| `mean_skip_null` | `2.0` — excluded from both sum and count |
| `mean_strict` | `Err(NullInAggregate)` |
| `mean_zero_null` | `1.333…` |

- `_skip_null` is the planner default; the plan summary states it.
- Two skeletons differing only in null handling are **different skeletons**. Merging them is exactly the silent-wrong-answer class this architecture exists to prevent.
- `null` compares false against everything including `null`. `is_null` is the only test. No three-valued logic.
- The renderer always reports exclusion counts when non-zero: *"Average of 4,812 records; 193 had no value."*

## 2.4 Counting

| Op | `[1, null, 3]` | Meaning |
|---|---|---|
| `count_rows` | `3` | cardinality of the collection |
| `count_values` | `2` | non-null values in a projected field |

**There is no bare `count`.** It is exactly the name whose meaning readers assume and get wrong. `mean_skip_null ≡ sum_values / count_values`, stated in the catalog rather than left to implementation. Both return `Ok(0)` over empty — zero is the correct cardinality of nothing, unlike the mean of nothing.

## 2.5 Empty versus error

Three outcomes, distinguished to the renderer.

| Outcome | Meaning | Renderer |
|---|---|---|
| `Ok(value)` | computed | states the value |
| `Ok(Empty{reason})` | well-defined, no matching records | "No records matched" + the emptying filter |
| `Err(e)` | failure | error, never a number |

**`Empty` never coerces to zero.** `div(Empty, 5)` is `Empty`. "No data" becoming "the value is zero" is a wrong answer wearing a correct answer's clothes.

`Empty` carries the narrowest filter that produced it, so the renderer can say "no people over 28" rather than "no results."

## 2.6 Time

- All internal time is **UTC epoch milliseconds**. No exceptions in the kernel.
- Every tenant declares a **reporting timezone** (IANA identifier), stored in the schema graph, versioned.
- `bucket(field, granularity, tz)` takes tz explicitly, bound from the tenant default at plan time and **recorded in the binding**. Never implicit.
- The **tzdb version is pinned in the kernel**, shipped rather than read from the host, and included in the system version hash. Historical replays use the tzdb version recorded in the trace.
- DST-ambiguous instants resolve to the **earlier** offset. Specified, not left to a library default.
- Calendar granularities are defined against the tenant timezone and the proleptic Gregorian calendar. Fiscal calendars are a per-tenant offset declaration.
- All intervals are half-open `[start, end)`.

## 2.7 Relative time as a bound hole

```rust
struct TemporalBinding {
    expression: RelativeExpr,   // LastNDays(30) | CalendarQuarter(-1) | MonthToDate | ...
    granularity: Granularity,
    tz: IanaTz,
    resolved: Interval,         // from journaled now()
    resolved_at: u64,
}
```

- **Resolution** at plan construction from a journaled `now()`, so replay is exact.
- **Cache key** stores the *relative* form plus a validity bucket computed **in tenant timezone**, calendar-aware:
  ```
  validity_bucket = calendar_bucket(now_utc, granularity, tenant_tz)
  ```
  Arithmetic `floor` division is wrong above `Hour`. A UTC bucket for an `Asia/Kolkata` tenant turns over at 05:30 local, serving yesterday's window during a five-and-a-half-hour band every morning.
- **Binding and ledger** record `resolved`, so an audit shows which window was computed.

## 2.8 Ordering and collation

All string ordering is **byte-wise over UTF-8**. No locale, no ICU, no implicit normalisation. Deterministic across every platform, and it is what `Ord for str` already does.

Human-meaningful ordering is a **presentation concern** applied by the renderer after the deterministic result is fixed. It never affects `take`, `sort_by` inside a harness, or `output_hash`.

`normalize_nfc(str)` exists as an explicit op. Never applied implicitly — implicit normalisation is another way for two byte-different inputs to collide.

## 2.9 Canonical serialisation for hashing

A dedicated module with its own test suite. **Not `serde_json::to_vec`** — hashing and display have different requirements and must not share a function.

- Floats: **shortest round-trip** (Ryū). Pinned implementation, version-locked, determinism-tested.
- `NaN` is forbidden in output; an op producing it returns `Err`. There is no correct hash for a value unequal to itself.
- `-0.0` normalises to `0.0`. Infinities serialise as `"Infinity"` / `"-Infinity"`.
- Decimals serialise as `(unscaled_int, scale)`, never as a string.
- Map keys sort byte-wise before hashing.
- **Errors in the value path are structured enums with no free-form text.** A `Debug` rendering containing a pointer, thread id, duration, or `HashMap` would enter `output_hash`. Human strings are produced by the renderer, after hashing.

## 2.10 Determinism hygiene in kernel code

Three defects that pass every local test and surface only in the nightly replay job.

- **`std::collections::HashMap` iteration order is randomly seeded per process.** `BTreeMap`/`BTreeSet` everywhere in the value path, hashing path, and effect path. Enforced by clippy `disallowed_types` with a small annotated allow-list for order-irrelevant internal caches.
- **`serde_json`'s `preserve_order` feature** switches `Map` from `BTreeMap` to `IndexMap`. If any transitive dependency enables it, key order becomes the customer database's whim. Deny it in `Cargo.toml`; assert key ordering when canonicalising tool output.
- **Fast-math flags.** Never enabled anywhere. CI check fails the build if `-ffast-math` or an equivalent appears.

Cross-architecture determinism test in CI on x86_64 and aarch64: `sum` over a fixed 10,000-element float vector must produce a byte-identical result, debug and release.

---

# PART III — LANGUAGE AND EXECUTION

## 3.1 Substrate

`starlark-rust`. Python's syntax with a DSL's semantics.

**Against a bespoke DSL:** direct emission into an unfamiliar grammar underperforms emitting Python and converting. Every custom grammar pays an out-of-distribution tax.

**Against plain Python:** the restriction is the product. An end user's phrasing causes an LLM to author a program running against a customer's production API with the customer's credentials. Arbitrary Python is uninsurable — halting is undecidable, side effects unbounded from the first `import`.

## 3.2 Inherited guarantees

| Property | Consequence |
|---|---|
| No recursion | Call graphs are DAGs by construction |
| No `while`; `for` only over finite materialised iterables | Loops terminate |
| No I/O, no arbitrary `import` | Effect surface is exactly the injected builtins |
| Module globals frozen after load | No shared mutable state |
| Dicts insertion-ordered by spec | No hash-order nondeterminism |
| Hermetic evaluation | Same inputs → same outputs |

```toml
[starlark]
dialect_version = "1"
enable_recursion = false
enable_sets = false
enable_top_level_stmt = false
enable_f_strings = true
enable_lambda = true
globals_frozen = true
```

Assert the config hash at startup. An accidentally-enabled flag silently voids I1.

## 3.3 Op catalog

| Family | Signature | Members |
|---|---|---|
| `agg_scalar_{int,float,decimal}` | `(T[]) -> R` per §2.2 | `sum` `mean_*` `median` `min` `max` `stddev` |
| `agg_multi` | `(num[]) -> num[]` | `mode` `quartiles` |
| `agg_param` | `(num[], num) -> num` | `percentile` `trimmed_mean` |
| `count` | `(T[]) -> Int` | `count_rows` `count_values` |
| `cmp_binary` | `(T, T) -> bool` | `eq` `ne` `gt` `gte` `lt` `lte` |
| `cmp_range` | `(T, T, T) -> bool` | `between` `outside` |
| `arith_{int,float,decimal}` | `(T, T) -> R` | `add` `sub` `mul` `div` `div_floor` `mod` `pow` |
| `coll` | `(T[], fn) -> T[]` | `filter` `map` `sort_by` `group_by` `take` `skip` `distinct` |
| `semijoin` | `(T[], U[], key) -> T[]` | `exists_in` `not_exists_in` |
| `str` | | `concat` `lower` `trim` `split` `format` `normalize_nfc` |
| `pred` | `(any) -> bool` | `is_null` `is_num` `is_str` `is_list` |
| `time` | | `bucket` `to_epoch` `date_part` |
| `declass` | `(tainted T) -> Int` | `declassify_count` |

**Signature family determines parameterisability** (§6.2) and therefore hit rate. Members of a family share a signature exactly; anything differing in arity or return type belongs to a different family.

Partiality is explicit: division by zero, empty aggregates, and null handling return `Result`, never raise.

## 3.4 Host builtins — the complete effect surface

```python
call_tool(name, **kwargs) -> Result[Value]
map_tool(name, arg_list) -> Result[list[Value]]      # parallel, index-ordered, all-or-nothing
call_harness(ref, **kwargs) -> Result[Value]
call_model(prompt, schema) -> Result[Value]           # budget defaults to 0
now() -> Int                                          # journaled
emit(event, payload) -> None                          # closed enum, scalar payload, untainted only
fail(reason) -> !                                     # constant or declassified reason only
```

Nothing else. No filesystem, network, environment, or randomness outside the effect system.

## 3.5 Budgets — single depleting pool

One pool per turn, passed by mutable reference, depleted by every frame. Per-frame budgets let ten nested harnesses consume ten times the ceiling.

```rust
struct Budget {
    steps: AtomicU64,        // 100_000
    heap_bytes: AtomicU64,   // 64 MiB
    wall_ms: AtomicU64,      // 30_000
    effects: AtomicU32,      // 64
    tool_ms: AtomicU64,      // 20_000
    model_calls: AtomicU32,  // 0
    depth: AtomicU32,        // 16
    result_rows: AtomicU64,  // 1_000_000
    join_hops: AtomicU32,    // 3
}
```

Audit executions (§14.3) carry a **separate pool** so an audit cannot cause a user-facing timeout.

On exhaustion: structured error naming the dimension and the frame. **Never partial output** — a partial aggregate is indistinguishable from a correct one at the presentation layer.

## 3.6 Execution modes

```
Live    : effects dispatched; results journaled
Replay  : effects read from journal; never dispatched
Shadow  : reads dispatched; WRITES HARD-STUBBED; output computed, not served
Dry     : all effects stubbed from contract sample data
```

## 3.7 Async bridging

```
tokio ─ spawn_blocking ──> Starlark evaluator (sync)
                                │ builtin invoked
                                ▼
                     mpsc::send(EffectRequest{ resp: oneshot })
                                │ blocking_recv
                                ▼
  Effect driver (async) ─ principal ─ taint ─ dispatch ─ journal ─ oneshot::send
```

Do not attempt continuation-based interpreter suspension; it is unsupported. One blocking thread per concurrent execution to ~1k concurrency; above that, pool with work-stealing and cap in-flight executions rather than threads.

This is why **no Node sidecar exists in the data plane** — an IPC hop from inside a blocking evaluator thread adds latency and a failure mode inside the kernel.

---

# PART IV — COMPOSITION

## 4.1 Content addressing

```
harness_hash = BLAKE3(rendered_source ‖ system_version_hash)
```

where `system_version_hash` covers dialect, op catalog, **and renderer** (§17.1). Immutable. Names resolve to hashes at plan construction; the hash executes.

## 4.2 Cycle prevention

- **Static:** graph reachability — `MATCH (h)-[:CALLS*]->(h)` non-empty ⇒ reject.
- **Runtime:** explicit call stack of hashes; a repeat → `PermissionDenied("cycle")`.

Both. Static catches build-time; runtime catches dynamically-constructed refs.

## 4.3 Example

```python
# sum_field → 3a9f…
def main(records, field):
    return ops.sum(ops.project_non_null(records, field))

# average_field → 7c21…
def main(records, field):
    total = call_harness("3a9f…", records=records, field=field)
    n = ops.count_values(records, field)
    if n == 0:
        return Empty("no non-null values")
    return ops.div(total, n)

# filtered_average → e10b…
def main(source, filters, target_field):
    records = call_tool(source, **pushdown_args(filters))
    kept = ops.filter(records, lambda r: all_match(r, filters))
    return call_harness("7c21…", records=kept, field=target_field)
```

Holes: `source`, `filters` (variadic), `target_field`. Structure: the fetch → filter → delegate chain.

## 4.4 Replay semantics

Two properties customers will conflate:

- **Computation reproducibility (I1):** replaying a trace with its journal produces byte-identical output. Always guaranteed. This is what audit means.
- **Answer reproducibility:** re-executing against live data later. **Not guaranteed and not desirable.**

Name them differently in the product: "Verify computation" versus "Re-run."

## 4.5 Nightly replay verification

The highest-value test in the system. Sample production traces, re-execute in `Replay`, assert every step hash and the final hash match. Divergence means a nondeterminism bug. Hard alert. A "flaky" replay is I1 failing, which voids everything above it.

---

# PART V — EFFECTS, CONTRACTS, SECURITY

## 5.1 Tool contracts

```rust
struct ToolContract {
    name: String, version: u32,
    input_schema: JsonSchema, output_schema: JsonSchema,
    effect_class: EffectClass,       // Read | Write | IdempotentWrite
    completeness: Completeness,      // Complete | Paginated{cursor_key, max} | Sampled | Unknown
    determinism: Determinism,        // Deterministic | Volatile | Nondeterministic
    totality: Totality,
    pushdown: Vec<PushdownCapability>,
    max_result_rows: Option<u64>,
    stable_order: bool,
    idempotency: Option<IdempotencyKeySpec>,
    sample_data: Option<Value>,
    returns_entity: EntityId,
    row_scoped: bool,
}
```

**`Completeness::Unknown` and `Completeness::Sampled` are promotion blockers for any workflow computing an aggregate.** A mean over a sample is a legitimate statistic but not the answer to "what is the average," and must never be presented as one. `Sampled` sources remain usable for existence checks and for top-k with an explicit annotation.

## 5.2 Result ordering and pagination

**The kernel canonically orders every tool result** by the entity's declared key, byte-wise, before it enters the value graph. Unconditional, not configurable. An entity without a declared key cannot back an aggregate-reachable tool.

Sorting the returned page is insufficient for paginated sources — a different arbitrary page each call, sorted consistently, is a deterministic ordering of a nondeterministic sample. Therefore:

- **Paginated contracts declare a `cursor_key`** forming a total order. Registration rejects a paginated tool without one.
- **The kernel drives pagination**, requesting successive pages until exhaustion or budget. A harness never sees a page.
- **Page continuity is verified**: each page's first cursor value must strictly follow the previous page's last. A violation is `UnstableSource` — the source mutated mid-scan or the ordering is not total — and fails loudly rather than aggregating an inconsistent snapshot.

`stable_order: true` is **verified in Shadow** by hashing pre- and post-sort. `Determinism::Deterministic` is **probed** in Shadow by calling twice with identical arguments; a mismatch auto-downgrades to `Volatile` and notifies the admin. Do not trust an unverified assertion a safety property depends on.

## 5.3 Effect classification and duplication

| Rule | Enforcement |
|---|---|
| `effect_class` declared at registration | SDK refuses registration without it |
| `Shadow`/`Dry` **hard-stub all writes** | Effect driver, pre-dispatch, not overridable |
| Write-containing workflows execute only in `Promoted` | Promotion gate + runtime |
| Every write carries an idempotency key | §5.4 |
| Writes **never** auto-retry | Effect driver |
| >1 write requires non-atomic acknowledgement | Promotion gate |
| **Audit path never re-executes writes** | §14.3 |
| **Equivalence merging never executes writes** | Golden cases are read-only by construction |

Stubbing is **loud** — synthetic success plus a `Stubbed` journal entry — so shadow comparison excludes write-dependent branches rather than reporting permanent spurious divergence.

## 5.4 Idempotency keys

```
key = (turn_key, effect_seq, element_index?)
```

`turn_key` is client-supplied and stable across retries (§10.1). `element_index` is present for `map_tool` elements and absent for scalar effects — without it, all N elements of a `map_tool` share a key and a downstream idempotent-write tool deduplicates them into one.

## 5.5 `map_tool`

**All-or-nothing.** Any element failing fails the whole call, naming failing indices and causes. Partial results into an aggregate are the same failure class as pagination truncation.

- Retries per element for `Read` and `IdempotentWrite`; never for `Write`.
- Concurrency bound is part of the dialect config, so in-flight count is deterministic.
- Journal entries written in **input-index order** regardless of completion order.
- `map_tool_lenient` returns `list[Result[V]]`, forcing the harness to handle each case — visible in the AST, visible to Phase 3, subject to promotion review. Never the default.

## 5.6 Effect set — derived by graph, trusted by hash

Derivation is a reachability query. **The result is not trusted live.** A stale index, incomplete traversal, or optimiser bug would become an authorisation bypass.

1. At promotion, materialise the effect set and store it in the immutable segment keyed by `signature`.
2. At execution, the kernel reads the **content-addressed cached value**, never the graph.
3. The kernel walks the AST it is about to execute and asserts its direct tool references are a subset of the cached set. Mismatch → hard fail, alert, quarantine.

The graph *derives*; the content-addressed cache is what you *trust*. Step 3 is one AST walk and turns an index bug into a loud failure rather than a silent escalation. Enforced structurally by `Verified<T>` (§17.2).

## 5.7 Principal-scoped authorisation

```rust
struct Principal {
    tenant_id: TenantId,
    subject: Subject,          // TenantAdmin | EndUser{external_id}
    grants: CapabilitySet,
    row_scope: Option<Value>,
    issued_at: u64, expires_at: u64,
    signature: Signature,
}
```

- **Kernel enforces capability** — may this principal reach this tool at all. Checked pre-execution against the complete effect set. Refusal is total.
- **Tool enforces rows** — which records. The kernel passes `row_scope` opaquely.

`row_scope` is a **required positional** on any `row_scoped` tool, so forgetting authorisation is a type error rather than a silent leak. Document the division loudly; a customer assuming the kernel does row filtering will build a hole.

For **join paths**, grants are checked on tools returning **every entity on the path**, not just the source. Checking only the source makes path discovery a privilege-escalation primitive.

## 5.8 Authoring tiers

| Tier | Who | May match | May author | Effects |
|---|---|---|---|---|
| **Run** | End user | `Promoted` only | no | per grants |
| **Explore** | End user, opt-in | `Canary` + `Promoted` | `Read`-only, ephemeral, **not persisted** | `Read` |
| **Design** | Tenant admin | all | yes, persisted | `Read` + `Write` |

Default to **Run**. This also decouples authoring cost from end-user traffic.

## 5.9 Taint tracking

Every value carries a taint bit; any op with a tainted input produces tainted output. Tool outputs are tainted; constants and verified hole bindings are not.

Tainted values **may not**: be a `call_tool`/`call_harness` name; appear in a `call_model` prompt; appear in an authoring prompt; influence a permission decision; be interpolated into an idempotency key; determine a join path; **appear in an `emit` payload**; **appear in a `fail` reason**.

They **may** be computed on freely and appear in final output, escaped and provenance-marked by the renderer.

`declassify_count(x) -> Int` returns cardinality only — useful telemetry without a leak, visible in the AST, subject to promotion review.

## 5.10 Partial failure

- **Zero-write:** the overwhelming majority. No issue.
- **Single-write:** idempotency key makes at-most-once safe. Permitted.
- **Multi-write:** **forbidden by default.** Requires explicit non-atomic acknowledgement; the trace records it and the ledger flags every execution.

Carry a `compensation_status` field per write journal entry from v0 even though unused, so adding sagas later is not a migration.

---

# PART VI — ABSTRACTION AND REUSE

## 6.1 What a skeleton is

A normalised harness AST with every parameter-classified subterm replaced by a typed hole. Two requests are the same workflow iff their skeletons are isomorphic.

## 6.2 The parameter boundary rule

> A subterm is a **hole** iff substituting any other value of the same type yields an AST isomorphic to the original. Otherwise it is **structure**.

| Subterm | Class | Reason |
|---|---|---|
| `28` | hole `threshold` | swap for 30 → identical graph |
| `height` | hole `field` | swap for weight → identical graph |
| `gt` | hole `op: cmp_binary` | family shares `(T,T)->bool` |
| `between` | **structure** | arity 3 |
| `mean_skip_null` | hole `agg: agg_scalar_T` | family shares signature |
| `mean_strict` | **structure** | different null semantics |
| `percentile` | **structure** | arity 2 |
| `mode` | **structure** | returns `num[]` |
| fetch→filter→aggregate | **structure** | the essence being preserved |

Over-parameterise and you converge on a general-purpose program with a form UI. Under-parameterise and the cache never hits.

## 6.3 Extraction pipeline

```
AST (from authoring, or parsed for hand-written harnesses)
  │
  ├─ 1 NORMALISE
  │     α-rename locals to positional
  │     canonicalise commutative operand order by subterm hash
  │       — INTEGER, BOOLEAN, AND STRING OPERANDS ONLY.
  │         Float and Decimal are never reordered; source order is structure.
  │     strip comments and docstrings
  │     inline single-use bindings
  │     reorder independent statements only where independence is proven
  │
  ├─ 2 ABSTRACT      apply §6.2; assign hole ids; record type, role, consumes[]
  ├─ 3 HASH          signature = BLAKE3(skeleton ‖ system_version_hash)
  ├─ 4 EQUIVALENCE   §6.5 — merge if behaviourally equivalent
  └─ 5 STORE         Workflow record, status = Draft
```

**Float reordering breaks I1.** Floating-point addition is not associative; reordering a summation changes the low bits. Unrestricted commutative reordering would make two skeletons hash identically and compute differently, and §6.5 would happily alias them.

Additionally: float aggregates use **pairwise summation** with recursion order fixed by input index, and input order is always the canonical order from §5.2.

**Be conservative in step 1.** A wrong reorder silently merges two distinct workflows. A fragmented cache is recoverable; a merged-wrong cache is not.

## 6.4 Hole specification

```rust
struct HoleSpec {
    id: HoleId,
    type_constraint: TypeConstraint,
    semantic_role: SemanticRole,
    required: bool,
    variadic: bool,
    discriminating: bool,        // §7.7 — op-valued holes
    default: Option<Value>,
    consumes: Vec<ConstraintKind>,
    join_path_allowed: bool,
}
```

## 6.5 Behavioural equivalence merging

At promotion, test a `Draft` against existing skeletons with the same hole arity and type profile over the golden set plus generated inputs. Agreement on all cases records an **alias**, not a new signature.

Golden cases are **read-only by construction** — the SDK rejects a golden case whose effect set contains a write, or equivalence testing fires it. Write workflows are compared structurally only.

Aliasing rather than deletion preserves audit. Cap the input budget; this is a heuristic, not a proof, and the evidence blob says so.

## 6.6 Cold start and seeding

Seed skeletons for shapes recurring in every deployment: filter-aggregate, group-aggregate, top-k, time-bucket, count-distinct, two-source semi-join, ratio, threshold-count.

**Role-abstracted exemplars.** Seeds carry `{source: <ENTITY:Person>, agg: <AGG_SCALAR>, filter: <FIELD:numeric> <CMP> <VALUE>}`, not concrete nouns. One tenant says "people," another "customers," another "members" — a seed embedded with concrete vocabulary matches none of them and the cold-start mitigation quietly does nothing.

**Vocabulary-normalised retrieval.** Phase 1 maps entity and field names through the schema graph to canonical entity roles and semantic types before embedding. Entities declare a `canonical_role` (`Person`, `Transaction`, `Product`, `Event`, `Location`, `Organisation`) at registration, defaulted by field-shape heuristic.

**Two vector queries per retrieval** — tenant exemplars and role-normalised seeds — merged before Phase 2. Both are HNSW lookups.

**Seed hit rate is a distinct metric.** If seeds never match, §6.6 is decoration and you should know in week two.

---

# PART VII — MATCHING

## 7.1 The governing asymmetry

A plain LLM turn fails **noisily and independently**. A false cache hit fails **silently and repeatedly** — the kernel confidently runs `mean` when the user meant `median`, deterministically, at scale, with no model in the loop. Reuse converts a stochastic error into a systematic one. **Tune for precision, never recall.**

## 7.2 Pipeline

```
structured intent
  │
  ├─ PHASE 0: FAST PATH
  │    round-trip check on the extraction (§8.2) — ALWAYS RUNS
  │    normalised intent hash → plan cache
  │    hit ⇒ execute; no match verification
  │
  ├─ PHASE 1: RETRIEVE       one hybrid query, §7.3 — top-k = 8, never top-1
  │
  ├─ PHASE 2: BIND + GATES   structural, HARD GATE
  │    a) extract hole values
  │    b) typecheck against HoleSpec
  │    c) resolve Field holes against schema graph, including join paths (§13)
  │    d) confirm required holes filled
  │    e) RESIDUE CHECK (§7.4)
  │    f) FAN-OUT CHECK (§13.4)
  │    g) UNIT COMPATIBILITY (§13.6)
  │
  ├─ PHASE 3: VERIFY         one model call, precision gate
  │    focused prompt for discriminating holes (§7.7)
  │    explicit confirmation required — absence of objection is not confirmation
  │
  └─ EXECUTE (populate Phase 0)   or   AUTHOR
```

**Phase 0 cache key:** `BLAKE3(normalised_intent ‖ tenant_id ‖ schema_version ‖ join_path_set ‖ grant_set_hash ‖ validity_bucket ‖ tenant_tz)`.

`grant_set_hash` matters because without it two principals with different grants share an entry; the second gets an authorisation error rather than falling through to a workflow they *could* run. On authorisation failure against a cached plan, **fall through to Phase 1** rather than erroring.

**Phase 0 entries expire** — 30-day TTL plus invalidation on schema, grant, timezone, or retirement change.

## 7.3 Phase 1 as one hybrid query

```
MATCH (w:Workflow)
WHERE w.status IN $tier_permitted
  AND w.embedding_key = $current_key
  AND NOT EXISTS { (w)-[:REQUIRES_TOOL]->(t) WHERE NOT (t)<-[:GRANTS]-(:Principal {id:$p}) }
VECTOR_SEARCH w.intent_embedding NEAR $intent_vec
LIMIT 8
```

This **retrieves candidates; it does not authorise** (I5). If the anti-join is wrong, the worst case is a candidate failing authorisation later.

## 7.4 The residue check

**The most important single check in the system.**

*"Average height of people over 28 in Bangalore."* A skeleton with a single filter slot binds cleanly — every required hole filled. Phase 3 may confirm, because the skeleton *does* compute an average height of people over 28. The city constraint vanishes and the user gets a confident, precise, wrong number.

Decompose intent into constraints. **Every constraint must be consumed** — by a hole whose `consumes` covers its kind and which was bound from it, or by structure that provably encodes it. **Any unconsumed constraint rejects the match unconditionally.** No model call, no override, no confidence threshold.

## 7.5 Variadic holes

The residue check is what makes variadic holes safe. `filters: list[FilterSpec]` lets a two-filter request bind with a two-element list, consuming both — a significant hit-rate win. Without residue checking, a variadic hole silently swallows extras, which is worse than having none.

Variadic where the op is associative and order-independent — filters, group keys, projections. Fixed-arity where order matters.

## 7.6 Fail closed

If no candidate clears Phase 3, author. **Never bind the nearest neighbour because nothing better exists.**

The temptation to relax this when hit rate disappoints is the most dangerous impulse in the project, and it will arrive as a reasonable optimisation with a supporting dashboard. Hit rate improves via §6.2 families, §7.5 variadicity, §13 join paths, and §8.2 extraction — never by loosening the gate.

## 7.7 Discriminating holes

Op-family holes buy hit rate and **concentrate risk**. `mean` versus `median` produces the same signature, binds the same holes, consumes the same constraints. Residue cannot distinguish them. Phase 3 is the only gate.

- Holes are **discriminating** (op-valued: `agg`, `cmp`, quantifiers) or non-discriminating (values, fields, sources).
- **Phase 3 uses a separate focused prompt for discriminating holes**, naming alternatives: *"This will compute the **mean**. Alternatives: median, sum, min, max. Is mean correct?"* A narrow question with named alternatives is answered far more reliably than "does this workflow answer this request."
- Any plan whose discriminating holes were bound from **implicit** language ("typical," "usual," "normal," "middle") shows the plan before executing. "Average" is explicit for `mean`; the others are not. Grow the implicit-term list from correction data.
- The audit over-samples discriminating bindings.

**If audits show discriminating-hole errors dominating, split the family** — separate signatures for `mean` and `median` — trading hit rate for structural discrimination. Decide now that you will pull this lever; it is much harder under pressure.

## 7.8 Confidence-gated presentation

| Condition | Behaviour |
|---|---|
| Phase 0 hit, read-only, promoted | execute silently |
| Phase 3 confirmed, read-only, >100 executions, no join path, explicit discriminating binding | execute silently |
| <100 executions, **or any join path**, **or implicit discriminating binding** | execute, show plan summary |
| Any write | **show plan, require explicit confirmation** |
| `corrected_bindings` returned | show corrected plan, require confirmation |

Join paths always surface: *"Filtered on `city` via `people → primary_address`"* is the sentence that lets a user catch a wrong path at a glance.

## 7.9 Negative cache

Record every rejected `(intent, candidate, phase, reason, residue)`. Highest-signal dataset you will have.

**Residue stores constraint kinds and field roles, never values** — `{"kind":"filter","field_role":"Categorical","consumed":false}`. An unconsumed constraint is `{"field":"city","value":"Bangalore"}`, and this is a dataset §7.9 tells you to actively grow, in the control plane, in Private mode. Assisted mode may store values.

---

# PART VIII — CONDUCTOR AND AUTHORING

## 8.1 Triage

- **`Reply`** — conversational, no execution.
- **`Compute`** — requires a harness.
- **`Clarify`** — underdetermined; ask exactly one question.

`Clarify` is the pressure valve keeping ambiguity out of the library. Without it, ambiguous requests become permanently-cached ambiguous workflows.

**The `Reply` path is constrained**, because everything in this document is downstream of triage being correct and triage was the one ungated component:

- **`Reply` may not contain numeric claims about tenant data.** A post-check scans for numerals, currency symbols, percentages, and quantity words near entity or field names from the schema graph. A hit forces re-triage as `Compute`. High recall; false positives cost one re-triage.
- **`Reply` never touches tools.** Enforced structurally: no effect-driver access on that path.
- **Triage is versioned and evaluated** like the extractor, with a labelled eval set and a regression gate on model change.
- **Triage decisions enter the ledger** with `path: Reply` and a response hash. Otherwise "why did the system tell my user X" has no answer when the answer came from triage.
- **Bias toward `Compute`.** A wrong `Compute` costs one wasted match attempt that falls through anyway. A wrong `Reply` produces a fabricated number.

## 8.2 Structured intent extraction

Emits a constraint set, never free text. Schema versioned and pinned into `embedding_key`.

Embed the **structured intent**, never the raw utterance — "average height of people over 28" and "median height of people over 28" are near-identical in embedding space and completely different workflows.

**Round-trip check runs on every turn, including Phase 0 hits.** Render the constraint set back to natural language, compare against the utterance. This validates the *extraction*, which Phase 0 otherwise skips — and a cached extraction error is permanent and deterministic, hit by every future identical utterance with no gate at all.

Schema-constrained decoding makes structural validity free.

## 8.3 Authoring — AST as JSON

Grammar-constrained decoding of Starlark text is a local-inference feature; hosted APIs offer JSON-schema structured outputs only.

```
1 PLAN      step plan from constraints, restricted to reachable ops, tools, entities
2 EMIT AST  model returns a JSON AST under strict schema. Node types from a closed
            enum: Call, OpRef, HarnessRef, Let, For, If, Literal, HoleRef, Lambda.
3 RENDER    Rust deterministically renders AST → Starlark. Pure function.
4 STATIC    §9 — effect set, permissions, DAG, pushdown, fan-out, units, budget
5 DRY RUN   Dry mode over contract sample data
6 REPAIR    structured diagnostics only. CAP AT 3. Then Clarify or plain turn.
7 EXTRACT   §6.3 — no parse step; the AST already exists
```

The entire syntax-error class disappears; the renderer being pure makes authored programs canonical by construction. **Generate the AST schema from the op catalog** rather than hand-writing it, so they cannot drift.

## 8.4 The real risk

Structured emission removes syntactic failure. The remaining risk is **semantic** — programs that render cleanly and compute the wrong thing. Nothing in authoring catches "averaged the wrong column." Only golden cases do. Budget for them from day one.

## 8.5 Rendering the answer

- Output values are tainted; escape and provenance-mark.
- The renderer receives the plan summary including resolved join paths, null exclusion counts, and unit conversions.
- **Never let the renderer recompute or adjust a number.** If it can do arithmetic, determinism ends at the last layer, invisibly. Constrain it to templating over supplied values.

## 8.6 Renderer fidelity

Phase 3 shows the model a rendering of the skeleton. If that rendering diverges from execution semantics, you verify a description rather than the program.

- The verification renderer is a **pure function of the same AST the kernel executes**, sharing the traversal with the Starlark renderer.
- A property test asserts that for any AST the rendering mentions every effect, discriminating binding, join path, null-handling decision, unit conversion, and `take`/`limit`. Mechanically checked, not eyeballed.
- Anything unrenderable is a **hard failure**, never a silent omission.
- The rendering shown to the user is the **same rendering** shown to the verifier. Two renderings drift; one cannot.

---

# PART IX — STATIC ANALYSIS AND PROMOTION

## 9.1 Mandatory checks: `Draft` → `Shadow`

| Check | Failure |
|---|---|
| Renders and parses under pinned dialect | reject |
| Symbols resolve against builtins + reachable tools | reject |
| No cycles | reject |
| Effect set ⊆ principal grants | reject |
| Effect-set assertion holds (I5) | reject + alert |
| No `Unknown`/`Sampled` completeness under an aggregate | reject |
| Pushdown satisfied | reject |
| All join paths non-multiplying and simple | reject |
| Join hops ≤ 3 | reject |
| Unit compatibility across aggregates | reject |
| Depth ≤ 16 | reject |
| Write count ≤ 1 | reject unless acknowledged |
| Taint reachability (incl. `emit`, `fail`) | reject |
| `call_tool` inside `for` | **warn**, suggest `map_tool` |
| Budget estimate within pool | warn + approval |

## 9.2 Pushdown analysis

For each `call_tool` whose result is filtered in Starlark:

- exceeds `result_rows` **and** a matching pushdown exists → reject, push the predicate into the call
- exceeds budget, no matching capability → reject: *"source too large to filter client-side; tool needs parameter X"*
- `max_result_rows` unknown → reject; contract incomplete

The second message turns a runtime failure into a design-time task, telling the customer exactly which tool needs a parameter.

## 9.3 Two promotion routes

Volume is one path to confidence, not the definition of it. A workflow matched monthly reaches 500 canary executions in four decades — and since writes require `Promoted`, **no low-frequency write workflow could ever run** under a volume-only ladder.

**Route A — Volume** (head workflows)

| Stage | Gate |
|---|---|
| Shadow → Canary | ≥50 executions, divergence <1% |
| Canary → Promoted | ≥500 executions, audit disagreement <0.5%, p99 within SLO |

**Route B — Evidence** (tail workflows)

| Stage | Gate |
|---|---|
| Draft → Canary | all §9.1 checks; ≥5 golden cases incl. ≥2 adversarial, all passing; clean Dry run; **admin approval** |
| Canary → Promoted | ≥10 executions, zero divergence; admin confirmation; **90-day re-review** |

- Route selected automatically from observed frequency after 14 days in Shadow; admins may force Route B earlier.
- **Any write workflow requires Route B regardless of volume.**
- Route B workflows carry a visible `evidence_promoted` flag in the plan summary and ledger.
- Unreviewed Route B workflows drop to Canary at 90 days.

## 9.4 Promotion transitions are control-plane-only

Two replicas both observing a threshold crossing would both promote, producing double events, inconsistent state, or lost `stats` updates.

Transitions are control-plane-only, guarded by compare-and-swap on `(signature, current_status)`. **Replicas report raw counters; they never transition state.** Threshold evaluation happens once against a complete count rather than N times against N partial counts.

## 9.5 Shadow comparison

- **New workflow:** reference is the authoring-path result on the same input.
- **Revised:** reference is the previous promoted version.
- **No reference:** shadow validates only that it does not crash. **Say so explicitly in the dashboard.** Do not let "shadow passed" imply correctness was verified.

Divergence scoring excludes stub-dependent branches.

## 9.6 Contract-change invalidation

```
MATCH (t:Tool {name:$tool})-[:HAS_CONTRACT]->(c:Contract {version:$old})
MATCH (w:Workflow)-[:REQUIRES_TOOL]->(t) WHERE w.pinned_contract_version = $old
RETURN w
```

Affected workflows drop to `Draft` pending re-validation. Run synchronously on registration — a customer bumping a contract should see the blast radius immediately, not discover it when answers change.

---

# PART X — TURN LIFECYCLE

## 10.1 Turn idempotency

Kernel-internal retries are covered by §5.4. **Client retries are the more common case and were unaddressed** — a phone loses signal, the app retries, a new turn gets a new trace, the keys differ, the write fires twice.

- **Every turn carries a client-supplied `turn_key`**, a UUID stable across retries. The chat endpoint requires it; the reference client generates one per user submission, not per HTTP attempt.
- Idempotency keys derive from it (§5.4).
- A **turn-result cache** keyed by `turn_key`, 24-hour TTL, returns the stored result without re-execution.
- A repeated `turn_key` with a **different body** is a client bug and is rejected loudly. Silently serving the first result would be worse.
- `turn_key` enters the ledger, distinguishing "the user asked twice" from "the network retried."

## 10.2 Session serialisation

Two turns racing on one session collide on ledger `seq` and `prev_hash`.

- Turns within a session **serialise** behind a per-session mutex held by the owning replica (§10.4).
- Queue depth bounded, default 2. Beyond that, reject with "still processing your previous message" rather than building a backlog of stale computations.
- Ledger append happens **inside** the lock, after execution, so `prev_hash` is always the true predecessor.
- **`Reply` entries append asynchronously** in a batched writer — they gate nothing, and blocking the cheapest path on a session lock is a bad trade. Marked `unordered: true` so verification does not treat them as chain links.
- Plan-cache writes are idempotent and need no locking.

## 10.3 Grant revocation mid-execution

- **Reads:** pre-execution check stands; the exposure window is bounded by `wall_ms`. Document it explicitly.
- **Writes:** re-check the grant immediately before **every** write. Rare, cheap, and executing a write against a revoked grant is materially different from reading stale data.
- **Token expiry is checked before every effect.** Free — a timestamp comparison — and it bounds the common case of a token lapsing.
- Revocation invalidates `principal_cache` and the affected `grant_set_hash` partition of the Phase 0 cache.

## 10.4 Multi-instance ledger

Every real customer runs replicas. A per-session hash chain forks across them, and I4 is void.

- **Session ownership** by lease in a small shared store the customer already runs (Postgres, Redis, or their primary DB — SDK ships adapters). 5-minute TTL, renewed per turn.
- **Chain locality:** the owning replica holds the tip. Other replicas **forward** rather than appending locally.
- **Failover:** on lease expiry another replica claims ownership and starts a **new chain segment** whose genesis embeds the previous segment's final hash plus a `SegmentBoundary` marker. Verification walks segments in order. Continuity without distributed consensus for something that does not need it.
- **Trace and plan cache stay replica-local** — content-addressed and idempotent, so duplication is harmless. Only the ledger is order-dependent.
- **Single-instance deployments skip all of it.** If no shared store is configured, run single-instance and **refuse to start a second instance** against the same store rather than silently forking.

Verification tooling handles segments from day one. Retrofitting segment awareness after a customer has a year of forked chains is not a repair.

## 10.5 Schema version pinning

Schema version is pinned at plan construction, recorded in the binding, and read for the whole execution. A change takes effect for plans constructed after it; in-flight executions complete against their pinned version, and the ledger records which — otherwise a boundary-time answer is unexplainable.

---

# PART XI — TOPOLOGY

## 11.1 Three privacy modes

| Mode | Triage / extraction | Matching | Authoring | Verification | Rendering |
|---|---|---|---|---|---|
| **Private** (default) | data plane | control plane (hashes + vectors) | **data plane** | data plane | data plane |
| **Assisted** (opt-in) | data plane | control plane | control plane | control plane | data plane |
| **Isolated** | data plane | **local mirror** | data plane | data plane | data plane |

Authoring consumes the constraint set, and constraints contain values. A control-plane authoring service is therefore incompatible with the Private promise. **The authoring pipeline ships in the Rust SDK** — plan generation, AST emission, rendering, static checks, Dry run, repair. More code in the customer binary than originally assumed, and not optional.

Sell Assisted honestly: richer authoring and debugging in exchange for sharing query values.

Isolated mirrors the entire workflow library locally, losing cross-tenant seeds and centralised promotion. Cheap to support given §11.3's mirroring already exists, and some customers will require it.

**Embeddings still cross in Private mode.** A vector is not practically reversible to text, but say so explicitly in security documentation rather than leaving customers to wonder, and note the residual: embeddings leak coarse semantic information.

## 11.2 Plane split

```
┌───────────── CONTROL PLANE (your cloud) ──────────────┐
│ ASTROLOBE: skeletons · intent vectors · dependency     │
│   graph · schema graph · contracts · trace metrics ·   │
│   negative cache (kinds and roles, no values)          │
│ SERVICES: matcher · promotion · eval harness · anchors │
│ LANGUAGE: Rust core; TypeScript for evals, prompt      │
│   tooling, dashboards only — no runtime path           │
│ NEVER: credentials, hole values, tool outputs, rows,   │
│   concrete harness source                              │
└───────────────────────┬────────────────────────────────┘
                        │ mTLS, OUTBOUND-INITIATED
┌───────────────────────┴────────────────────────────────┐
│         DATA PLANE (customer backend — your SDK)       │
│ REDB: plan cache · turn cache · traces · ledger ·      │
│   tool registry · mirrored promoted plans · rendered   │
│   harness cache (local, evictable, never synced)       │
│ KERNEL: evaluator · effect driver · taint · principal  │
│   verification · credential vault · authoring pipeline │
│ LANGUAGE: Rust only. Single static binary.             │
└────────────────────────────────────────────────────────┘
```

**Outbound-initiated.** No inbound firewall rules, no IP allowlisting. Build it this way from the first commit.

## 11.3 Outage behaviour

| Capability | Offline? |
|---|---|
| Phase 0 fast path | **yes** |
| Promoted workflow execution | **yes** — plans mirrored |
| Phase 1 retrieval | local mirror only |
| Authoring | **yes in Private mode** (data-plane pipeline) |
| Promotion | no |
| Metrics | buffered |

A control-plane outage degrades to "no new promotions," not "product down."

## 11.4 Transmission allow-list

An explicit enumeration of field names permitted to cross to the control plane, enforced at the serialisation boundary, with a test that **fails on any un-listed field**. Structural enforcement, so the next person adding a field makes a deliberate decision.

## 11.5 SDK surface

```python
from harness import tool, serve, entity, Completeness, EffectClass

@entity("Person", key="id", canonical_role="Person")
class Person:
    id: str
    name: str
    age: int
    height: Length(unit="cm")
    primary_address_id: str        # N:1 — traversable
    order_ids: list[str]           # 1:N — semi-join only

@tool(
    description="Fetch people records",
    effect_class=EffectClass.READ,
    completeness=Completeness.PAGINATED(cursor_key="id", max_page=500),
    pushdown=["age.gt", "age.gte", "city.eq", "joined_at.between"],
    max_result_rows=2_000_000,
    returns_entity="Person",
    stable_order=False,
    row_scoped=True,
)
def get_people(row_scope: RowScope, min_age: int | None = None,
               city: str | None = None) -> list[Person]:
    return db.query(...).filter(tenant_visible(row_scope))

serve()
```

Each contract field closes a specific hole documented above. **The SDK docs should say which**, or customers experience them as bureaucracy and fill them in carelessly.

## 11.6 MCP adapter

Not your ingest path — your procurement answer. An adapter importing MCP tool definitions into contracts (with `completeness: Unknown` and `effect_class` requiring manual declaration, since MCP carries neither) costs a week now and a quarter later.

---

# PART XII — STORAGE

## 12.1 Why a unified engine earns its place

| Modality | Job |
|---|---|
| Vector | intent embeddings for Phase 1 |
| Graph | harness dependency DAG **and** tenant schema graph |
| Full-text | tool and skeleton discovery during authoring |
| Columnar | metrics over trace metadata |

The compounding case is §7.3: one query touching vector similarity, graph reachability, and scalar predicates. In a split stack that is three round trips and a correctness question about composition.

## 12.2 Immutable / mutable segmentation

**Immutable — content-addressed, write-once, no MVCC**

| Store | Key | Value |
|---|---|---|
| `skeleton_blob` | `signature` | canonical skeleton, hole schema |
| `effect_set` | `signature` | materialised effect set (**I5 trust anchor**) |
| `contract_blob` | `contract_hash` | full contract at a version |
| `golden_case` | `case_hash` | constraints, expected output, adversary kind |

Put-if-absent; no version chains, no vacuum, no visibility checks. A duplicate write is a no-op by definition.

**Note: `harness_blob` does not exist in the control plane.** Concrete source contains user literals — `city == "Bangalore"` — and storing it there violates Private mode in the schema itself. Concrete harnesses are **rendered data-plane-side** from `skeleton + binding` by the same deterministic renderer. Identity is `(signature, binding_hash)`, not a harness hash.

**Mutable — MVCC**

`workflow_state` (status, stats, promotion history, pinned contract versions), `skeleton_alias`, `principal_grant`, `schema_graph`, `tool_current`.

Only this segment pays transaction cost, and it is a small fraction of bytes and read volume.

## 12.3 Vector store

```
workflow_intent { signature, constraints, embedding, embedding_key }
```

**Partition by `embedding_key`.** A model swap writes a new partition, both stay live during migration, cutover is a partition switch. HNSW; tune for latency and let k=8 absorb imperfect recall, since Phases 2 and 3 are the real gates.

## 12.4 Graph — dependency

```
(:Harness)-[:CALLS]->(:Harness)
(:Harness)-[:USES_TOOL]->(:Tool)
(:Workflow)-[:INSTANTIATES]->(:Harness)
(:Workflow)-[:REQUIRES_TOOL]->(:Tool)
(:Tool)-[:HAS_CONTRACT]->(:Contract)
(:Principal)-[:GRANTS]->(:Tool)
```

Makes cheap: transitive effect set, cycle detection, contract invalidation, permission anti-join, retirement blast radius. All reachability.

## 12.5 Columnar — trace metrics

Metadata only, never payloads: timestamp, tenant, signature, principal kind, phase reached, residue outcome, join hops, step and effect counts, budget consumption, outcome, audit flag.

Retention: full metadata 90 days, rolled-up aggregates indefinitely.

## 12.6 Data plane — `redb`

```
plan_cache      intent_hash → { signature, bindings, join_paths, schema_ver, hits }
turn_cache      turn_key    → { result, body_hash, expires }
promoted_plans  signature   → { skeleton, holes, effect_set }
harness_render  (sig, binding_hash) → rendered source        (local, evictable)
trace           trace_id    → { steps, journal, hashes, outcome, is_audit }
ledger          seq         → LedgerEntry
tool_registry   name        → contract
principal_cache principal_id→ verified grants + expiry
```

`promoted_plans` and `plan_cache` are what make offline operation work. Trace retention is customer-configurable, default 30 days; the ledger is retained longer since it is small and is the audit artifact.

## 12.7 Ledger

```rust
struct LedgerEntry {
    seq: u64, prev_hash: ContentHash,
    session_id: Uuid, turn_id: Uuid, turn_key: Uuid,
    principal_id: PrincipalId,
    path: Path,                          // Reply | Cached | Matched | Authored | Clarified
    skeleton_sig: Option<ContentHash>,
    bindings_hash: Option<ContentHash>,
    join_path_hash: Option<ContentHash>,
    temporal_hash: Option<ContentHash>,
    journal_hash: Option<ContentHash>,
    output_hash: Option<ContentHash>,
    effect_classes: BTreeSet<EffectClass>,
    system_version: ContentHash,
    schema_version: u64,
    non_atomic_ack: Option<AckRef>,
    evidence_promoted: bool,
    unordered: bool,                     // Reply entries
    ts_ms: u64,
    entry_hash: ContentHash,
}
```

Chain in the data plane; periodic Merkle roots anchor to the control plane — tamper-evidence without the control plane seeing content.

`join_path_hash` and `temporal_hash` are not optional. If an answer depended on traversing `people → primary_address` over a `LastNDays(30)` window, the audit record must say so, or a schema change makes historical answers unexplainable.

---

# PART XIII — SCHEMA GRAPH AND JOIN-PATH BINDING

## 13.1 The problem

Resolving field holes only against fields **directly on the source entity** means *"average height of people in Bangalore"* fails whenever `city` lives on an `address` entity — even though the shape is exactly the filter-aggregate skeleton already in the library. In any realistic schema, most interesting filters live one or two hops away. Not an edge case; the common case.

## 13.2 Model

```
(:Entity {name, key, canonical_role})-[:HAS_FIELD]->(:Field {name, storage_type, semantic_type, unit, nullable})
(:Entity)-[:RELATES_TO {cardinality, join_key, name}]->(:Entity)
(:Tool)-[:RETURNS]->(:Entity)
(:Tool)-[:CAN_PUSHDOWN {predicate}]->(:Field)
```

`cardinality ∈ {ONE_TO_ONE, MANY_TO_ONE, ONE_TO_MANY}` is **mandatory**. Registration rejects a relationship without it.

## 13.3 Binding with paths

```
resolve(hole, constraint, source_entity):
    direct = source_entity.fields[constraint.field]
    paths  = find_simple_paths(source → any entity having field, max_hops=3)
    paths  = [p for p in paths if is_non_multiplying(p)]
    paths  = [p for p in paths if principal_may_reach_every_entity(p)]

    candidates = ([direct] if direct else []) + paths
    if len(candidates) == 0: return Unresolved
    if len(candidates) >  1: return Ambiguous(candidates)
    return candidates[0]
```

A `PathBinding` compiles into additional fetch-and-join steps. **The skeleton is unchanged** — path resolution is part of the binding, not the structure. That is what makes the win large: one skeleton serves every filter reachable within three hops.

**Paths must be simple.** Self-referential relationships (`Employee → manager → Employee`) are traversable but each entity may appear at most once per path. Otherwise `Employee → manager → Employee → name` binds silently to a different `name` than the direct one.

**A direct field and a path-reachable field sharing a name is ambiguity**, not a preference for the direct one. "Manager's name" and "employee's name" are both plausible readings of "name" in a request about employees.

Resolved paths are recorded in the binding, the Phase 0 cache key, the plan summary, and the ledger.

## 13.4 Fan-out prevention

Traversing a 1:N edge multiplies rows. Join `people → addresses` where someone has three addresses and the mean height counts them three times — precise, confident, and wrong.

| Cardinality | Traversable? |
|---|---|
| `ONE_TO_ONE` | yes |
| `MANY_TO_ONE` | yes |
| `ONE_TO_MANY` | **no join.** Requires an explicit quantifier compiling to a **semi-join** |

A path is non-multiplying iff **every** edge is 1:1 or N:1. Checked in Phase 2f **and** re-checked in static analysis, because a schema change can turn a safe path multiplying.

For 1:N the extractor produces a quantified constraint compiling to `exists_in`, filtering without multiplying. If phrasing does not determine the quantifier — "people with orders in Bangalore" could mean any, all, or most recent — that is a `Clarify`, not a guess.

## 13.5 Ambiguity

`person → billing_address → city` and `person → shipping_address → city` are both non-multiplying, both type-correct, and mean different things.

**Do not tie-break. Do not prefer shortest.** Return `Ambiguous`; Phase 2 rejects; the conductor Clarifies naming both. Deterministic tie-breaking here is a silent-wrong-answer generator with a plausible rule in front of it.

A tenant may configure a **preferred path** per `(entity, field)`, recorded in the binding and ledger. An explicit customer decision, not an inference.

## 13.6 Units and semantic types

Fields carry `semantic_type ∈ {Length, Mass, Money, Duration, Count, Ratio, Identifier, Categorical}` and a `unit` (required when dimensional).

- **Aggregating mismatched units is rejected in Phase 2**, alongside residue and fan-out.
- Unit conversion is an **explicit op** in the skeleton, visible to Phase 3 and to the user. Never implicit.
- `Money` requires a currency and forces `Decimal`. Cross-currency aggregation without an explicit conversion op is rejected — the conversion needs a rate, the rate needs a date, and neither should be inferred.
- `Identifier` and `Categorical` are **not aggregatable**. Averaging an ID is always a mistake and the type system should say so.
- `Ratio` is not summable; averaging one is flagged, since the mean of ratios is usually not the ratio of means. Show both and let the user choose.

## 13.7 Schema drift

On schema change, run synchronously and report affected counts:

1. Invalidate Phase 0 entries whose `join_path_set` touches a changed edge.
2. Re-run `is_non_multiplying` over stored bindings — an N:1 → 1:N change **retires** every binding through that edge.
3. **Re-check ambiguity.** Adding a `shipping_address` relationship changes no existing edge but makes `person → * → city` ambiguous where it was unique. Previously-unambiguous bindings must retire rather than keep executing on the old assumption.

---

# PART XIV — OBSERVABILITY AND AUDIT

## 14.1 Metrics

| Metric | Target | Meaning if bad |
|---|---|---|
| Fast-path hit rate | >30% steady | queries less repetitive than assumed |
| Match rate | >60% by month 3 | economic case unproven |
| **Skeleton reuse ratio** (cross-tenant) | **>20:1** | **kill criterion** |
| Binding reuse ratio (per tenant) | >5:1 | fast path underperforming |
| **Audit disagreement rate** | **<0.5%** | correctness broken — stop shipping features |
| Residue rejection rate | 5–20% | too low ⇒ check not working; too high ⇒ holes insufficiently variadic |
| Join-path bind rate | rising then plateau | measures the schema-graph lift directly |
| Path ambiguity rate | <10% | schema needs preferred-path config |
| Fan-out rejections | >0 is healthy | zero suggests the check is not firing |
| Seed hit rate | >0 by week two | otherwise cold-start mitigation is decoration |
| Authoring cost | declining | plan step too verbose |
| Repair convergence | >85% within 3 | catalog or contracts unclear to the model |
| **Replay divergence** | **0** | I1 violated; everything above is void |

**Replicas emit raw counters, never ratios.** With N replicas each holding a local cache, averaging N local ratios is wrong; the correct figure sums numerators over summed denominators. Getting this wrong makes the kill criterion read better than reality — the worst possible metric to bias optimistically.

**Skeleton and binding reuse are separated deliberately.** A skeleton may show 40:1 cross-tenant while each tenant sees 1:1. Report both or you will misread your own business.

## 14.2 The kill criterion

If every interaction produces a bespoke program executed once, you have built a compiler whose output runs a single time. Instrument from week one. If skeleton reuse sits near 1:1 after cold start, the thesis is falsified and no engineering fixes it. **Build the measurement before the optimisations.** Report cold start (first 30 days per tenant) separately.

## 14.3 Measuring false hits in production

A target you cannot measure is a target you will report as met. Golden cases test what you thought of; production false hits are by construction the ones you did not.

**1. Shadow-author audit (primary).** Sample 1–2% of matched requests, run the authoring path on the same intent, compare.

- **Read-only workflows:** execute both, compare outputs.
- **Write workflows: structural comparison only** — same skeleton family, same discriminating bindings, same effect set — **with no execution of either.** Re-executing a write workflow to measure correctness would make the audit a duplicate-side-effect generator on a sampling schedule in production.
- Divergence on a write workflow queues for human review at elevated priority.
- `Dry` mode gives output comparison without effects where sample data exists.
- Audits use a **separate budget pool**, are **tagged in the trace**, and are **excluded from `stats`** — otherwise a workflow promotes on traffic that was never real.

Weight sampling toward newly-promoted signatures, Route B workflows, join-path binds, **fast-path hits** (least-gated), and **discriminating bindings** (structurally undetectable errors).

**2. Implicit signals.** Rephrasing within 60 seconds, explicit corrections, abandonment. Not proof — correlate against sampled audits to calibrate.

**3. Explicit correction path.** One click on every answer showing a plan summary, capturing trace, skeleton, bindings. Every correction becomes a golden-case candidate, **invalidates the Phase 0 entry immediately**, and quarantines the intent hash. Highest-value data you can collect for the price of one button.

## 14.4 Golden case and stat granularity

Dangerous confusions live *within* a signature, between bindings. Per-signature stats cannot tell you a signature is fine for `sum` and broken for `median`.

- Golden cases key on `(signature, discriminating_binding_shape)`.
- `stats` shard the same way.
- Promotion gates evaluate **per binding shape**. A signature is promoted for the shapes it has evidence for; a novel discriminating binding enters at Canary **for that shape**.

Without the last rule, a signature promoted on 10,000 `sum` executions serves its first `median` request at full autonomy with zero evidence.

## 14.5 Adversarial golden set

| Adversary | Example |
|---|---|
| Aggregation swap | mean vs median vs mode |
| Comparison boundary | `>` vs `>=` |
| Interval inclusivity | Q3 inclusive vs exclusive |
| Field swap | height vs weight, same shape |
| Source swap | people vs employees |
| **Extra constraint** | +"in Bangalore" — residue check |
| Dropped constraint | omits a required filter |
| **1:N fan-out** | aggregate over a to-many path — must produce semi-join |
| **Path ambiguity** | billing vs shipping city — must Clarify |
| **Self-referential path** | "name" where manager exists — must Clarify |
| Order sensitivity | filter-then-aggregate vs reverse |
| Null semantics | skip vs strict vs zero |
| **Unit mismatch** | cm and inches in one aggregate — must reject |
| Pagination | full population vs first page |
| **Relative time rollover** | same query across tenant midnight |

Every one should retrieve the same candidate in Phase 1 and die in Phase 2 or 3. Without adversarial cases, the false-hit rate is unmeasured and the central claim is unfalsified.

---

# PART XV — THREAT MODEL

| Threat | Mitigation |
|---|---|
| Cross-user data access | principal-scoped capability + tool-enforced `row_scope` |
| Privilege escalation via authoring | authoring tiers; Run cannot author; Explore is read-only, non-persistent |
| **Authorisation bypass via stale index** | I5 — content-addressed effect set, AST-walk assertion, `Verified<T>` |
| **Join-path escalation** | grants checked on tools returning **every** entity on the path |
| Prompt injection via tool output | taint tracking incl. `emit` and `fail` |
| Prompt injection via utterance | schema-constrained extraction; conductor emits constraint sets |
| **Data exfiltration via `emit`** | closed event enum, scalar-only payloads, taint prohibition, `declassify_count` |
| **Data exfiltration via error text** | `fail` reasons must be constant or declassified |
| **Data exfiltration via negative cache** | residue stores kinds and roles, never values |
| **Data exfiltration via stored source** | control plane holds skeletons only; source rendered data-plane-side |
| **Data exfiltration via authoring** | authoring is data-plane in Private mode |
| Resource exhaustion | single budget pool, `result_rows`, `join_hops`, per-principal rate limits |
| Unintended writes | `Promoted` + Route B + explicit confirmation |
| **Duplicate writes** | stubbed outside `Promoted`; `(turn_key, effect_seq, element_index)`; no auto-retry; audit never re-executes |
| Library poisoning | Explore never persists; Design requires admin; write promotion requires approval |
| **Schema-graph inference** | resolution failures return generic `Unresolved`, never "no path from X to Y" |
| Model swap re-partitions library | `embedding_key` partitioning; migration with golden-set comparison |
| Contract drift | versioning, pinning, synchronous invalidation query |

---

# PART XVI — FAILURE MODES

| Mode | Detection | Response |
|---|---|---|
| False cache hit | audit, golden set, corrections | retire signature, tighten holes, add golden case |
| Residue too permissive | extra-constraint cases fail | audit `consumes`; over-broad variadic hole |
| Residue too strict | rejection rate >20% | make holes variadic where the op is associative |
| Fan-out corruption | 1:N golden case; implausible aggregates | check not firing; audit cardinality declarations |
| Ambiguity resolved silently | ambiguity rate ≈0 on a multi-address schema | tie-breaking crept in; remove it |
| Effect-set assertion failure | §5.6 step 3 | hard alert, quarantine, **audit the index** |
| Replay divergence | nightly job | hard alert; I1 void until fixed. Check `HashMap` usage first |
| Unstable source | page continuity check | `UnstableSource`; source needs a total order |
| Budget exhaustion | counters | structured error; never partial output |
| Tool unavailable | effect driver | fail closed; never substitute |
| Repair non-convergence | attempt counter | cap at 3, escalate to Clarify |
| Skeleton explosion | reuse ≈1:1 post-cold-start | revisit families, variadicity, `join_path_allowed`; if unfixable, thesis falsified |
| Over-abstraction | false-hit spike on one signature | split the skeleton — a hole was structure |
| Schema drift | Phase 2c failures rising | §13.7; invalidate Phase 0 |
| Ledger fork | verification finds a broken link | check session ownership; single-instance guard |
| Control plane unreachable | connection monitor | degrade per §11.3; **do not fail open on permissions** |

---

# PART XVII — VERSIONING AND STRUCTURAL CLOSURE

Two defect generators are closed by construction rather than by discipline. See `structural_closures.rs`.

## 17.1 System version

Every mutable input to a hash lives in one struct whose exhaustive destructuring **fails to compile** when a field is added without hashing it:

```rust
pub struct SystemVersion {
    pub op_catalog: Hash,       // Merkle root over per-op impl_hash — §17.3
    pub dialect: Hash,
    pub renderer: Hash,         // AST → Starlark; a change alters every harness hash
    pub tzdb: Hash,
    pub serialiser: Hash,
    pub intent_schema: Hash,
    pub extractor_model: Hash,
    pub embedding_model: Hash,
    pub triage_model: Hash,
}
```

`signature`, `harness_hash`, and every ledger entry carry `system_version.hash()`.

## 17.2 Verified authority

An effect set can only exist in a form the authoriser accepts if it was constructed by hash verification:

```rust
pub struct Verified<T>(T);                       // no public constructor
impl EffectSet {
    pub fn load(h: Hash, s: &Store) -> Result<Verified<EffectSet>>;   // verifies
}
pub fn authorize(e: &Verified<EffectSet>, p: &Principal) -> Result<()>;
```

Passing a graph-query result to `authorize` does not compile. I5 becomes a type error rather than a review item.

## 17.3 Op implementation hashing

`op_catalog_version` covers adding or changing an op's semantics. It does **not** cover fixing a bug in `median`'s tie-breaking — behaviour changes, every skeleton using `median` computes something different, no signature changes, nothing invalidates, no audit shows a boundary.

Each op carries an `impl_hash` over its implementation; the catalog version is the Merkle root. A bug fix therefore bumps the catalog version and invalidates dependent skeletons. Correct and painful, which is why it must be structural rather than left to discipline.

## 17.4 Migrations

Changing the extractor, triage model, embedding model, intent schema, tzdb, renderer, serialiser, or op catalog is a **migration**, not a config change: re-extract or re-embed as applicable, run the full adversarial golden set against the new configuration, compare disagreement rates, keep both live during transition, then cut over.

---

# PART XVIII — BUILD ORDER

| M | Scope | Exit |
|---|---|---|
| **M1** | Kernel core; numeric tower + return types; null-explicit ops; `count_rows`/`count_values`; canonical serialisation; byte-wise collation; UTC + pinned tzdb; `BTreeMap` lint; `SystemVersion` (§17.1) | **cross-architecture determinism test passes on x86_64 and aarch64** |
| **M2** | Effects; canonical ordering; kernel-driven cursor pagination with continuity; empty-vs-error; `map_tool` all-or-nothing; constrained `emit`/`fail`; determinism probing | `Sampled` source blocked from aggregate promotion; tainted-collection `emit` rejected statically |
| **M3** | Composition; effect set keyed by signature with `Verified<T>` (§17.2); op `impl_hash`; pushdown | `authorize` on an unverified effect set fails to compile |
| **M4** | Astrolobe control plane; immutable/mutable split; skeleton-only storage; transmission allow-list; semantic types and units | allow-list test fails on an added field |
| **M5** | Conductor; **data-plane authoring pipeline**; constrained `Reply`; AST-as-JSON; shared renderer with property test | full authoring runs with no control-plane call in Private mode |
| **M6** | Skeleton + matching; residue check; float-safe normalisation; relative-time holes with tenant-tz buckets; grant-aware cache key; round-trip on fast path | "in Bangalore" rejected and re-authored; `LastNDays` turns over at tenant midnight |
| **M7** | Schema graph + join paths; fan-out prevention; simple-path enforcement; ambiguity → Clarify; role normalisation; schema pinning | "in Bangalore" **matches**; 1:N gives semi-join; billing-vs-shipping Clarifies |
| **M8** | Turn lifecycle: `turn_key` idempotency, session serialisation, grant re-check on writes | duplicate `turn_key` returns cached result without re-executing a write |
| **M9** | Instrumentation; read-only-safe audit; binding-shape stats; discriminating-hole gating; raw-counter aggregation | audit ≥1%, disagreement <0.5%, every disagreement triaged. **Go/no-go.** |
| **M10** | Promotion: dual-route ladder, CAS transitions, ledger with segments | monthly write workflow promotes via Route B |
| **M11** | Topology: multi-instance ledger, three privacy modes, SDK, MCP adapter | three replicas, mid-conversation failover, chain verifies end to end |

**M1 and M2 carry the determinism weight, and that ordering is not negotiable** — retrofitting float, time, collation, or ordering invalidates every trace and signature produced beforehand.

**M9 is the go/no-go.** Promotion machinery is worthless if the reuse thesis does not hold on your workload.

---

# PART XIX — DEFERRED BY DESIGN

Listed so a later pass does not mistake them for defects.

1. **Phase 3 concentration (§7.7).** A trade, not a bug. Resolves empirically via the audit. Decide now you will split families if audits show discriminating-hole errors dominating.
2. **Multi-turn state.** The residue check has no story for constraints arriving across turns. Needs new design, not an amendment. Keep session state out of harness closures so it remains addable.
3. **Cost attribution.** Who pays for authoring, how a tenant caps spend, what happens at the cap. Cheaper to design before a billing system exists than around one.
4. **Streaming.** Determinism and partial output are in tension. Revisit after M9; stream *rendering*, never *computation*.
5. **Compensation / sagas.** Journal carries `compensation_status` from v0 so adding it is not a migration.
6. **Learned matching.** The negative cache is a labelled dataset, but it moves precision from a structural gate to a learned one. If pursued, use it to *reject* faster, never to *accept* without Phase 3.
7. **Op catalog growth.** The first customer needing op N+1 is the real test. Append-only, versioned, justified by more than one customer. The discipline of refusing is the product.
8. **Cross-tenant skeleton sharing beyond seeds.** Structure leaks business logic. Opt-in plus legal review.
9. **Cardinality inference.** Customers will under-declare, but an inferred N:1 that is really 1:N produces exactly the fan-out corruption §13.4 prevents. Use inference only to flag suspected mis-declarations for review, never to bind.
10. **Approximate aggregation.** Sketches would help latency, but an approximate answer presented with the same confidence as an exact one is a new silent-error class. Plan summary and ledger must carry the error bound; Phase 3 must confirm acceptance.
11. **Tool result caching.** Contradicts §4.4 — a cached tool result makes a fresh execution silently stale. TTLs must come from contract-declared volatility and appear in the plan summary.
12. **Partial control-plane degradation.** §11.3 covers full outage; partial (retrieval up, promotion down; stale mirror) is unspecified and is the more common real condition.

---

# APPENDIX A — AMENDMENT HISTORY

| Origin | Finding | Now in |
|---|---|---|
| v2 L1 | Shadow dispatched real writes | §5.3 |
| v2 L2 | Per-tenant not per-principal permissions | §5.7, §5.8 |
| v2 L3 | No residue check | §7.4 |
| v2 L4 | Tool semantics invisible | §5.1 |
| v2 L5 | No taint tracking | §5.9 |
| v2 L6 | Per-frame budgets | §3.5 |
| v2 L7 | No parallelism | §3.4 |
| v2 L8 | Unversioned extractor | §17.1 |
| v2 L9 | Signature over-sensitivity | §6.5 |
| v2 L10 | No pushdown analysis | §9.2 |
| v2 L11 | No partial-failure story | §5.10 |
| v2 L12 | Cold start ignored | §6.6 |
| v2 L13 | Every match cost a model call | §7.2 |
| v2 L14 | Replay conflation | §4.4 |
| v3 H1 | Effect set from live query | §5.6, §17.2 |
| v3 H2 | Join fan-out | §13.4 |
| v3.1 A1 | Float reordering | §6.3 |
| v3.1 A2 | Relative time | §2.7 |
| v3.1 A3 | `Reply` bypass | §8.1 |
| v3.1 A4 | Seed vocabulary | §6.6 |
| v3.1 A5 | Null semantics | §2.3 |
| v3.1 A6 | Numeric tower | §2.1 |
| v3.1 A7 | Timezone | §2.6 |
| v3.1 A8 | Collation | §2.8 |
| v3.1 A9 | Float serialisation | §2.9 |
| v3.1 A10 | Tool ordering | §5.2 |
| v3.1 A11 | Empty vs error | §2.5 |
| v3.1 A12 | `map_tool` partial failure | §5.5 |
| v3.1 A13 | Promotion long tail | §9.3 |
| v3.1 A14 | False-hit measurement | §14.3 |
| v3.1 A15 | Multi-instance ledger | §10.4 |
| v3.1 A16 | Discriminating holes | §7.7 |
| v3.1 A17 | Units | §13.6 |
| v3.1 A18 | Cache key grants | §7.2 |
| v3.1 A19 | Renderer fidelity | §8.6 |
| v3.1 A20 | Golden case granularity | §14.4 |
| v3.2 B1 | Pagination sampling | §5.2 |
| v3.2 B2 | Audit re-executes writes | §14.3 |
| v3.2 B3 | Validity bucket timezone | §2.7 |
| v3.2 B4 | Authoring location | §11.1 |
| v3.2 B5 | Source with literals in control plane | §12.2 |
| v3.2 B6 | Turn idempotency | §10.1 |
| v3.2 B7 | `emit` exfiltration | §5.9 |
| v3.2 B8 | Concurrent turns | §10.2 |
| v3.2 B9 | Fast-path extraction errors | §8.2 |
| v3.2 B10 | `count` semantics | §2.4 |
| v3.2 B11 | Aggregate return types | §2.2 |
| v3.2 B12 | Grant revocation | §10.3 |
| v3.2 B13 | Schema pinning | §10.5 |
| Conv N1 | `HashMap` iteration order | §2.10 |
| Conv N2 | `serde_json` `preserve_order` | §2.10 |
| Conv N3 | Error text in `output_hash` | §2.9 |
| Conv N4 | Renderer unversioned | §17.1 |
| Conv N5 | Op implementations unversioned | §17.3 |
| Conv N6 | `map_tool` key collision | §5.4 |
| Conv N7 | Equivalence merging executes writes | §6.5 |
| Conv N8 | Promotion transition race | §9.4 |
| Conv N9 | Per-replica ratio aggregation | §14.1 |
| Conv N10 | Self-referential paths | §13.3 |
| Conv N11 | Negative cache stores values | §7.9 |
