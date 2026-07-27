# F3 — Compute op signature table (§9)

**Sources:** §9, §5 (cast matrix + checked arithmetic), §11.  
**Status:** pure elaboration.

Convention: all ops are **pure**. Checked i64 arithmetic: overflow ⇒ `Type`. ÷0 ⇒ `Type`. Non-finite float results ⇒ `Type`. Comparisons **type-strict** (int vs float requires cast). No op may produce NaN/±∞.

| Op | Args | Return | ReasonCodes | Edge cases |
|----|------|--------|-------------|------------|
| `add` | 2× (int\|float same) | same | `Type` | overflow; mixed types |
| `sub` | 2× same | same | `Type` | overflow; mixed |
| `mul` | 2× same | same | `Type` | overflow; mixed |
| `div` | 2× same | same | `Type` | ÷0; overflow; mixed |
| `mod` | 2× int | int | `Type` | mod 0; overflow |
| `abs` | 1× number | same | `Type` | i64::MIN abs overflow |
| `min`/`max` | 2× same numeric | same | `Type` | mixed types |
| `eq`/`ne` | 2× same type | bool | `Type` | cross-type without cast |
| `lt`/`le`/`gt`/`ge` | 2× same numeric | bool | `Type` | mixed |
| `and`/`or` | 2× bool | bool | `Type` | non-bool |
| `not` | 1× bool | bool | `Type` | |
| `concat` | ≥0 str | str | `Type` | non-str arg |
| `length` | 1× str | int | `Type` | char count (Unicode scalar) |
| `contains`/`starts_with`/`ends_with` | 2× str | bool | `Type` | |
| `pull` | path (Expr layer) | any | `Missing` | resolved in Expr against bag |
| `exists` | path (Expr) | bool | — | false on missing |
| `merge` | left, right, on_conflict | map | `Type`/`Shape` | default `error` on key clash |
| `drop`/`keep` | map + keys | map | `Type` | |
| `path_copy` | from, to | bag edit | `Missing`/`Type` | |
| `count` | list | int | `Type` | |
| `append` | list, item, max_items | list | `Budget.Size`/`Type` | max_items mandatory on producer |
| `first`/`last` | list | elem | `Missing`/`Type` | empty |
| `slice` | list, start, end, max_items | list | `Budget`/`Type` | bounds |
| `list_contains` | list, item | bool | `Type` | |
| `is_type` | value, type-tag | bool | `Type` | |
| `matches_format` | value, validator_id | bool | `Type`/`Shape` | validators registered (§5.4) |
| `blake3` | value | str hex | `Type` | over canonical form |
| `now`/`uuid`/`random` | 0 | str/int | — | **L0-C ledgered** (§12.2 INJECT) |

## Conformance vectors (minimum ≥3 per core op family)

Vectors live under `docs/vectors/compute_*.json`. Families covered: arithmetic happy/fail/overflow; compare type-strict fail; string; structure; list bounds.

## Implementation status (F-008, 2026-07-27)

Full §9 catalog implemented:
- **Pure** (`aelio-kernel/src/compute.rs`): numeric, compare/logic, string, `is_type`, `blake3`, and structure/list — `merge, drop, keep, path_copy, count, append, first, last, slice, list_contains`. `pull`/`exists` resolve in the Expr layer.
- **Effectful, ledgered** (`Backend::nondet`/`validate`, routed via `exec::eval_fx` at value-producing sites only): `now, uuid, random` (L0-C, `nondet_value` INJECT) and `matches_format` (`validate_result` INJECT). Replay injects both from the ledger, so a turn using them is bit-identical (`nondet_replay.rs`).
- `path_copy` refined to 3-arg `(container, from, to)` for purity — see FLAGS F-008 Decision Log.

## Completion check

| Check | Result |
|-------|--------|
| Every §9 catalog op implemented | **PASS** (pure in `compute.rs`; nondet/validate via `Backend`) |
| Vectors incl. failures for structure/list families | **PASS** (`docs/vectors/compute_*.json`: merge/merge_conflict/count/list_contains/slice/append_budget/first_empty/keep + div0) |
| Nondet/validate replay bit-identity | **PASS** (`nondet_replay.rs`) |
| Zero contradictions with §5.2 cast matrix | **PASS** (cast is conversion-layer; compute does not implement forbidden casts) |
| Type-strict comparison explicit | **PASS** |
