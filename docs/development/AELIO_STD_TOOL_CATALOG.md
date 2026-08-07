# Aelio Standard Tool Catalog (`std.*`)

First-party tools registered at boot. Pure tools execute locally in Rust via `ops/pure` — no tenant SDK round-trip.

`ToolSpec` has no `provider` field: the reserved `std.` id prefix is the only marker of a
first-party tool, which is why `Registry::register_tenant_tool` rejects tenant tools that claim
that namespace (F-029). Local execution is still gated — `require_allow` runs at `ReadOnly` risk
before every `std.*` invocation, so a tenant can deny individual first-party tools by capability.

**Rollout status**

| Phase | Namespace | Count | Status |
|-------|-----------|-------|--------|
| — | `std.math.*` (core) | 8 | **Shipped** |
| A | `std.text.*`, `std.validate.*` | 22 | **Shipped** |
| B | `std.data.*` | 14 | **Shipped** |
| C | `std.math.*` (extended) | 5 | **Shipped** |
| D | `std.control.*`, `std.logic.*` | 6 | **Shipped** |
| E | `std.ideation.*` | 5 | **Catalog only** (LLM-backed; not registered yet) |
| F | `std.ephemeral.*` | 2 | **Shipped** (runtime sub-agents; destroyed after invoke) |

Implementation: [`aelio-os/crates/aelio-agent/src/orchestration/stdlib/`](../../aelio-os/crates/aelio-agent/src/orchestration/stdlib/)

---

## D4 harness-core catalog correspondence

Frozen for D4: a `catalog-op` is hashed by `harness-core`'s per-op `IMPL_REV` Merkle
catalog and is the eventual native harness implementation. `bridge-only` remains an
agent stdlib dispatch until a same-semantics harness-core operation is introduced;
it is **not** covered by the harness `SystemVersion.op_catalog` yet.

| std tool | D4 classification |
|---|---|
| `std.math.add` | catalog-op `numeric.add`; bridge-only until native harness lowering replaces stdlib dispatch |
| `std.math.subtract` | catalog-op `numeric.sub`; bridge-only until native harness lowering replaces stdlib dispatch |
| `std.math.multiply` | catalog-op `numeric.mul`; bridge-only until native harness lowering replaces stdlib dispatch |
| `std.math.divide` | catalog-op `numeric.div`; bridge-only until native harness lowering replaces stdlib dispatch |
| `std.math.sum` | catalog-op `agg.sum_num`; bridge-only until native harness lowering replaces stdlib dispatch |
| `std.math.average` | catalog-op `agg.mean_*`; bridge-only until native harness lowering selects the numeric variant |
| `std.math.min` | bridge-only (retire when a type-strict harness `min` op lands) |
| `std.math.max` | bridge-only (retire when a type-strict harness `max` op lands) |
| `std.math.clamp` | bridge-only (retire when a type-strict harness `clamp` op lands) |
| `std.math.percent_of` | bridge-only (retire when a type-strict harness composition is admitted) |
| `std.math.modulo` | bridge-only (retire when a type-strict harness `modulo` op lands) |
| `std.math.abs` | bridge-only (retire when a type-strict harness `abs` op lands) |
| `std.math.compare` | bridge-only (retire when a type-strict harness compare op lands) |
| `std.text.length` | bridge-only (retire when a harness string/container length op lands) |
| `std.text.concat` | bridge-only (retire when a harness text concat op lands) |
| `std.text.join` | bridge-only (retire when a harness text join op lands) |
| `std.text.split` | bridge-only (retire when a harness text split op lands) |
| `std.text.split_words` | bridge-only (retire when a harness text split-words op lands) |
| `std.text.strip` | bridge-only (retire when a harness trim op lands) |
| `std.text.lower` | bridge-only (retire when a locale-pinned case op is admitted) |
| `std.text.upper` | bridge-only (retire when a locale-pinned case op is admitted) |
| `std.text.contains` | bridge-only (retire when a harness text contains op lands) |
| `std.text.starts_with` | bridge-only (retire when a harness text prefix op lands) |
| `std.text.ends_with` | bridge-only (retire when a harness text suffix op lands) |
| `std.text.regex_match` | bridge-only (retire when a pinned-regex harness op lands) |
| `std.text.regex_extract` | bridge-only (retire when a pinned-regex harness op lands) |
| `std.text.truncate` | bridge-only (retire when a Unicode-version-pinned truncate op lands) |
| `std.validate.email` | bridge-only (retire when a pinned validator op lands) |
| `std.validate.phone_e164` | bridge-only (retire when a pinned validator op lands) |
| `std.validate.url` | bridge-only (retire when a pinned validator op lands) |
| `std.validate.uuid` | bridge-only (retire when a pinned validator op lands) |
| `std.validate.in_range` | bridge-only (retire when a harness range validator lands) |
| `std.validate.in_enum` | bridge-only (retire when a harness enum validator lands) |
| `std.validate.matches_pattern` | bridge-only (retire when a pinned-regex validator lands) |
| `std.validate.normalize_phone` | bridge-only (retire when a pinned phone normalizer lands) |
| `std.validate.normalize_email` | bridge-only (retire when a pinned email normalizer lands) |
| `std.data.count` | catalog-op `agg.count_rows`; bridge-only until native harness lowering replaces stdlib dispatch |
| `std.data.first` | bridge-only (retire when a harness list-first op lands) |
| `std.data.last` | bridge-only (retire when a harness list-last op lands) |
| `std.data.take` | bridge-only (retire when a harness list-take op lands) |
| `std.data.append` | bridge-only (retire when a harness list-append op lands) |
| `std.data.pick` | bridge-only (retire when a harness map-pick op lands) |
| `std.data.omit` | bridge-only (retire when a harness map-omit op lands) |
| `std.data.merge` | bridge-only (retire when a harness map-merge op lands) |
| `std.data.keys` | bridge-only (retire when a harness map-keys op lands) |
| `std.data.values` | bridge-only (retire when a harness map-values op lands) |
| `std.data.filter_equals` | bridge-only (retire when a harness list-filter op lands) |
| `std.data.sort_by_field` | bridge-only (retire when a harness field-sort op lands) |
| `std.data.dedupe` | bridge-only (retire when a harness dedupe op lands) |
| `std.data.map_field` | bridge-only (retire when a harness path-projection op lands) |
| `std.control.coalesce` | bridge-only (retire when a harness coalesce op lands) |
| `std.control.default_if_missing` | bridge-only (retire when a harness default op lands) |
| `std.control.pick_branch` | bridge-only (retire when harness control lowering lands) |
| `std.logic.and` | bridge-only (retire when a harness bool-and op lands) |
| `std.logic.or` | bridge-only (retire when a harness bool-or op lands) |
| `std.logic.not` | bridge-only (retire when a harness bool-not op lands) |
| `std.ephemeral.adapt` | bridge-only (retire only if ephemeral synthesis becomes a versioned harness primitive) |
| `std.ephemeral.read_document` | bridge-only (retire only if document attach becomes a versioned harness primitive) |
| `std.ideation.brainstorm_options` | bridge-only external/model operation; never a pure catalog op |
| `std.ideation.rank_options` | bridge-only external/model operation; never a pure catalog op |
| `std.ideation.pick_best` | bridge-only external/model operation; never a pure catalog op |
| `std.ideation.summarize_bullets` | bridge-only external/model operation; never a pure catalog op |
| `std.ideation.classify_intent` | bridge-only external/model operation; never a pure catalog op |

---

## Phase A — Text & validation

### `std.text.*`

| Tool ID | Params | Output | Notes |
|---------|--------|--------|-------|
| `std.text.length` | `text` | `length` (int) | str/list/map length |
| `std.text.concat` | `parts` (list) | `text` | Join scalars |
| `std.text.join` | `parts` (list), `separator` | `text` | Join strings |
| `std.text.split` | `text`, `delimiter` | `parts` (list) | |
| `std.text.split_words` | `text` | `parts` (list) | Whitespace split |
| `std.text.strip` | `text` | `text` | Trim |
| `std.text.lower` | `text` | `text` | |
| `std.text.upper` | `text` | `text` | |
| `std.text.contains` | `text`, `needle` | `match` (bool) | |
| `std.text.starts_with` | `text`, `prefix` | `match` (bool) | |
| `std.text.ends_with` | `text`, `suffix` | `match` (bool) | |
| `std.text.regex_match` | `text`, `pattern` | `match` (bool) | |
| `std.text.regex_extract` | `text`, `pattern` | `groups` (list) | First capture group set |
| `std.text.truncate` | `text`, `max_chars` | `text` | Char-safe truncate |

### `std.validate.*`

| Tool ID | Params | Output | Notes |
|---------|--------|--------|-------|
| `std.validate.email` | `text` | `valid` (bool) | Format check |
| `std.validate.phone_e164` | `text` | `valid` (bool) | E.164 |
| `std.validate.url` | `text` | `valid` (bool) | http(s) |
| `std.validate.uuid` | `text` | `valid` (bool) | |
| `std.validate.in_range` | `value`, `lo`, `hi` | `value` | Number range |
| `std.validate.in_enum` | `value`, `allowed` (list) | `value` | |
| `std.validate.matches_pattern` | `text`, `pattern` | `value` | Regex |
| `std.validate.normalize_phone` | `text`, `region?` | `phone` | E.164 output |
| `std.validate.normalize_email` | `text` | `email` | Lowercase + check |

---

## Phase B — Data (lists & maps)

| Tool ID | Params | Output | Notes |
|---------|--------|--------|-------|
| `std.data.count` | `list` | `count` (int) | |
| `std.data.first` | `list` | `item` | |
| `std.data.last` | `list` | `item` | |
| `std.data.take` | `list`, `n` | `list` | First n items |
| `std.data.append` | `list`, `item` | `list` | |
| `std.data.pick` | `map`, `keys` (list) | `map` | Subset of keys |
| `std.data.omit` | `map`, `keys` (list) | `map` | |
| `std.data.merge` | `left`, `right` | `map` | Shallow merge |
| `std.data.keys` | `map` | `keys` (list) | |
| `std.data.values` | `map` | `values` (list) | |
| `std.data.filter_equals` | `list`, `field`, `value` | `list` | Maps in list where field == value |
| `std.data.sort_by_field` | `list`, `field`, `order?` | `list` | asc/desc |
| `std.data.dedupe` | `list`, `field?` | `list` | Unique by field or whole value |
| `std.data.map_field` | `list`, `path` | `values` (list) | Pluck path from each map item |

---

## Phase C — Math (extended)

| Tool ID | Params | Output | Notes |
|---------|--------|--------|-------|
| `std.math.clamp` | `value`, `lo`, `hi` | `result` | |
| `std.math.percent_of` | `part`, `whole` | `result` | (part/whole)*100 |
| `std.math.modulo` | `a`, `b` | `result` | |
| `std.math.abs` | `value` | `result` | |
| `std.math.compare` | `a`, `b` | `relation` (str) | lt/eq/gt |

---

## Phase D — Control & logic

| Tool ID | Params | Output | Notes |
|---------|--------|--------|-------|
| `std.control.coalesce` | `values` (list) | `value` | First non-null |
| `std.control.default_if_missing` | `value`, `default` | `value` | |
| `std.control.pick_branch` | `condition`, `if_true`, `if_false` | `value` | Deterministic if/else |
| `std.logic.and` | `a`, `b` (bool) | `result` | |
| `std.logic.or` | `a`, `b` (bool) | `result` | |
| `std.logic.not` | `value` (bool) | `result` | |

---

## Phase F — Ephemeral runtime sub-agents

When no pinned workflow or registered tool matches, orchestration falls back to a **runtime
sub-agent**: a synthesized pipeline of pure `std.*` steps that is minted, executed, and destroyed
within the same turn (RAII via `EphemeralScope`).

| Tool ID | Params | Output | Notes |
|---------|--------|--------|-------|
| `std.ephemeral.adapt` | `goal`, `context?` (map) | `result` | Heuristic pipeline synthesis from goal + slots |
| `std.ephemeral.read_document` | `document`, `query` | `result` | Document attach pattern; pipeline destroyed after read |

**Lifecycle:** `mint → run_pipeline → destroy`. Trace fields: `ephemeral_created`, `ephemeral_destroyed`
on `Orchestrate.Execute` step. Pass attached content via turn slots: `document_text`, `document`,
`attached_text`.

**Not promoted:** ephemeral pipelines are turn-scoped unless a separate promotion observation fires.

**Deterministic ids:** pipeline ids are `ephemeral.<seq>.<blake3(goal)[..12]>` — derived from the
scope sequence and the goal, never the clock. They stay inside `EphemeralScope` for the trace and
are deliberately kept out of node output, so `ledger[].result` replays bit-identically (F-030).

---

## Tool routing priority (end-to-end)

Orchestration never shadows client-registered tools, and it never *selects* one either. Choosing a
tenant tool is ProposePath's job, because that is where the choice is typechecked against declared
abilities. Orchestration only claims a turn it can serve entirely from first-party pure operations:

| Priority | Source | When |
|----------|--------|------|
| 1 | **Pinned std workflow** | Sealed pin matches (e.g. `average`) → local Rust |
| 2 | **Standard tool** | Unambiguous whole-word match for a pure local op → local Rust |
| 3 | **Ephemeral sub-agent** | Pure local task with no client effect → mint/run/destroy pipeline |
| 4 | **Fallthrough** | Everything else → Conductor starters, then cold ProposePath over the full ability set |

Matching at levels 2 and 3 is whole-word, not substring: "summarize" must not reach `std.math.sum`.
When no synthesized step fits, the ephemeral pipeline is **empty and refuses**, so the turn falls
through instead of answering the user with a restatement of their own question.

**Tenant tools inside a graph.** A verified TaskGraph node may name a tenant tool in its
`strategy_hint`. That call goes through `blocks/tool_call.rs::run_tool_call_block`, exactly like
the ability-path executor: policy check, `resolve_all` binding of declared params from turn slots,
ledger intent, a per-call idempotency key derived from user + tool + version + args, signature
capture, and response redaction. If a required param cannot be bound, execution returns
`GraphExecution::NeedUser` and the turn asks rather than invoking. The orchestrator never calls
`host.call_with_context` directly.

Implementation: `orchestration/tool_router.rs` and `orchestration/executor.rs`. Ephemeral context
includes the `client_tools` + `standard_tools` catalog so the synthesizer does not duplicate client
capabilities.

---

## Phase E — Ideation (catalog only; Phase E implementation)

LLM-backed — register with `ToolEffect::External`, budget caps, policy gate.

| Tool ID | Params | Output | Notes |
|---------|--------|--------|-------|
| `std.ideation.brainstorm_options` | `prompt`, `max_options` | `options` (list) | |
| `std.ideation.rank_options` | `options`, `criteria` | `ranked` (list) | |
| `std.ideation.pick_best` | `options`, `criteria` | `choice` | |
| `std.ideation.summarize_bullets` | `text`, `max_bullets` | `bullets` (list) | |
| `std.ideation.classify_intent` | `text`, `labels` (list) | `label` (str) | |

---

## Usage in TaskGraph

Orchestrator sets `strategy_hint` on each node:

```json
{
  "id": "filter_orders",
  "strategy_hint": ["std.data.filter_equals"],
  "inputs": [
    { "kind": "slot", "name": "orders" },
    { "kind": "literal", "value": "status" },
    { "kind": "literal", "value": "paid" }
  ]
}
```

---

## Adding a new standard tool

1. Add row to this catalog (correct phase table).
2. Implement in `orchestration/stdlib/specs.rs` + `invoke.rs`.
3. Add unit test in `orchestration/stdlib/tests.rs` or inline `#[cfg(test)]`.
4. Pure only — no network, no wall-clock `now`, no randomness.
