# F11 — Conformance vector format + initial pack (§27)

**Sources:** §27, §8, App E, App G.

## File format (`docs/vectors/*.json`)

```json
{
  "name": "identity_passthrough",
  "section": "§8.2 Identity",
  "plan": { "nid": "n1", "op": "Identity" },
  "initial_bag": { "x": 1 },
  "injected_ledger": null,
  "expected": {
    "kind": "bag_hash",
    "bag": { "x": 1 }
  }
}
```

`expected.kind` ∈ `bag_hash` | `err` | `park`.

- `bag_hash`: run plan; compare `value_hash(final_bag)` to `value_hash(expected.bag)`.
- `err`: `{ "code": "Type", "detail_contains"?: "…" }`.
- `park`: `{ "park_nid": "…" }`.

Optional `injected_ledger` for pure replay meta-vectors.

## Initial vectors (pack)

| name | section | covers |
|------|---------|--------|
| `identity_01` | §8.2 | Identity #1 |
| `const_01` | §8.2 | Const whole-bag write |
| `seq_01` | §8.2 | Seq happy |
| `branch_true` / `branch_false` | §8.2 | Branch |
| `loop_budget_iter` | §8.3 | max_iter ⇒ Budget.Iter |
| `map_atomic` | §8.3 | Map into |
| `filter_01` | §8.3 | Filter |
| `guard_entry_fail` | §8.3 | Guard.Violation |
| `park_resume_01` | §8.4 | park/resume |
| `call_into` | §8.4 | Call args-only |
| `sense_write_reject` | §8.4 | plan-time Policy |
| `tee_park_side_reject` | §8.4 | plan-time reject |
| `compute_div0` | §9 | Type |
| `compute_int_ne_float` | §4.3 | eq type-strict |
| `replay_meta_identity` | §12.3 | run twice bag_hash equal |

## Completion check

| Check | Result |
|-------|--------|
| Vectors executable JSON | **PASS** (`docs/vectors/` + harness) |
| Every testable §8 claim has a vector citing section | **PASS** (pack + golden login) |
| Identity is vector #1 | **PASS** |
