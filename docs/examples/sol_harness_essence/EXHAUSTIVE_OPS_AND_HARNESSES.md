# Exhaustive operations + Sol harness catalog

**Status:** living inventory (2026-08-04)  
**Code library:** `aelio-os/crates/aelio-kernel/src/sol_harness_lib.rs`  
**Store table:** `sol_harness_contracts` via `store_library(tenant)`

This file lists **everything available to assemble harnesses**, then the **saved harness contracts**.

---

## A. Sol control ops (L0-A) — closed set

These are the machine instructions. You compose them; you do not invent new control ops without Mother amendment.

| Op | Role |
|----|------|
| `Const` | Replace/set bag literal |
| `Identity` | No-op |
| `Seq` | Ordered steps |
| `Let` | Lexical bindings (unwind on complete) |
| `Branch` | if pred then/else |
| `Loop` | while + `max_iter` |
| `Try` | catch by reason prefix |
| `Fallback` | try steps until one succeeds |
| `Guard` | invariant entry/exit/both/each |
| `Budget` | cap calls / tokens / ms |
| `Timeout` | deadline on body |
| `Once` | idempotent region |
| `Park` | end turn; wait for wake |
| `Tee` | body + side (side_root isolated) |
| `Map` | map over list path |
| `Filter` | filter list by pred |
| `Call` | invoke registered target → `into` path |

Sugar (desugars before plan): `Switch`, `Pipe`, `Retry` — see `aelio-kernel/src/sugar.rs`.

---

## B. Pure compute / Expr fns (§9 / F3) — closed set

Used inside `pred`, `args`, `Let` bindings as `{"fn":"…","args":[…]}`.

### Arithmetic
`add` `sub` `mul` `div` `mod` `abs` `min` `max`

### Compare / logic
`eq` `ne` `lt` `le` `gt` `ge` `and` `or` `not`

### String
`concat` `length` `contains` `starts_with` `ends_with`

### Paths / structure
`pull` `exists` `merge` `drop` `keep` `path_copy`

### List
`count` `append` `first` `last` `slice` `list_contains`

### Meta
`is_type` `blake3` `matches_format`

### Ledgered nondet (L0-C)
`now` `uuid` `random`

---

## C. Demo / library Call targets (stubs for Sol harness demos)

Not Mother ISA — **registered Call ids** used by seed harnesses. Production replaces these with admitted ability/tool pins.

| Call id | Effect | Used by |
|---------|--------|---------|
| `express.say@1` | read | most reply harnesses |
| `understand.classify@1` | read | understand / semantic |
| `memory.search@1` | read | memory_attach |
| `context.attach@1` | write | memory_attach |
| `compute.hold@1` | pure | calc / list (hold Expr result into bag) |
| `tool.act_stub@1` | external | confirm_then_act, once_external_stub |
| `harness.invoke@1` | read | parent_waits_on_child, stack_*, pipelines (run child Sol, wait, return bag; **nested** OK) |

### Agent L1 ability names (for later lowering — not all wired as Sol Calls yet)

`Sense.Env` `Sense.Session` `State.Read` `State.Direction` `Understand.Extract` `Judge.Confidence` `Bind.ResolveAll` `Invoke.Call` `Express.Template` `Express.Synthesize` `Express.Ask` `Registry.Capabilities` `Learn.ProposePath`

---

## D. Saved Sol harness library (33+ contracts)

| id | Library | Sol ops featured |
|----|---------|------------------|
| `quick_reply` | reply | Seq, Call, concat |
| `full_reply` | reply | Seq, Call |
| `apologize_closed` | reply | Call |
| `understand_intent` | intent | Seq, Call |
| `wait_for_user` | control | Seq, Call, **Park** |
| `memory_attach` | memory | Seq, Call×3 |
| `calc_sum_gate` | calculation | Const, Call, **Branch**, add/gt |
| `calc_repeat_add` | calculation | Const, **Loop**, Call, add/lt |
| `calc_mul_div` | calculation | Const, Call, Branch, mul/div/eq |
| `list_filter_keep` | calculation | Const, **Filter**, Call, count |
| `string_contains_gate` | calculation | **Branch**, contains |
| `semantic_ack` | intent | Call, Branch, Park |
| `confirm_then_act` | tools/policy | Call, Park, Branch |
| `detect_fresh_utterance` | control | Branch, or/contains |
| `greet_then_offer_help` | reply | Seq, Call, Branch |
| `parent_waits_on_child` | control | **Call harness.invoke@1** (1-level wait) |
| `stack_leaf_c` | control | leaf of nested stack |
| `stack_mid_b` | control | invoke leaf C, wait, continue |
| `stack_top_a` | control | invoke mid B, wait (B→C nested) |
| `fallback_say` | control | **Fallback** |
| `guard_required_field` | control | **Guard**, exists |
| `once_external_stub` | tools | **Once**, Call |
| `budgeted_express` | control | **Budget**, Call |

(+ combination pipelines: `help_router`, `memory_then_full_reply`, `greet_memory_pipeline`, … — see `library/catalog.json`)

### Parent → child wait (and nested stack)

**`harness.invoke@1`:** runs another saved Sol harness synchronously; parent `into` receives the **child bag**. The **child registry also has invoke**, so mid harnesses can call grandchildren while the top waits. Depth capped by `HARNESS_INVOKE_MAX_DEPTH` (8). Parked children not supported in this demo.

Full write-up: [`NESTED_HARNESS_STACK.md`](./NESTED_HARNESS_STACK.md)

Proof:

```bash
cargo test -p aelio-kernel parent_harness_calls_child_waits_for_output
cargo test -p aelio-kernel nested_harness_stack_a_waits_on_b_waits_on_c
```

---

## E. How to store / load

```rust
use aelio_kernel::{sol_harness_library, store_library, load_contract, compile};
use aelio_store::MemoryStore;

let mut store = MemoryStore::new();
store_library(&mut store, "demo")?;
let c = load_contract(&store, "demo", "calc_repeat_add")?.unwrap();
compile(&c.program_json)?;
// sol_harness_library().len() is 33+ (includes stack_top_a / mid / leaf)
```

```bash
cd aelio-os
cargo test -p aelio-kernel sol_harness
```

JSON mirror: `docs/examples/sol_harness_essence/library/`

---

## F. Still incomplete (Gate HS)

- Conductor does not yet select/play these Sol contracts in the live turn path
- Call stubs ≠ production ability/tool registry
- Expanding **control/compute ISA** requires Mother amendment — expanding **harnesses** and **Call targets** does not
