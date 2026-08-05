# Sol harness store → retrieve → replay (essence demo)

**Status:** working kernel proof (2026-08-04)  
**Code:** `aelio-os/crates/aelio-kernel/tests/sol_harness_store_replay.rs`  
**Rule:** sugar names → Sol ops; durable contract = Sol JSON in store

## What this proves

```text
natural-language prompt
  → (mock) LLM devises Sol steps
  → store contract in DB (MemoryStore / Aelio DB trait)
  → retrieve by id
  → execute via aelio-kernel
  → replay ledger → same bag_hash
  → retrieve again → reuse → same bag_hash
```

## Run

```bash
cd aelio-os
cargo test -p aelio-kernel --test exhaustive_harness_system
```

Full matrix: [`EXHAUSTIVE_SYSTEM_TEST_MATRIX.md`](./EXHAUSTIVE_SYSTEM_TEST_MATRIX.md)  
Nested stack: [`NESTED_HARNESS_STACK.md`](./NESTED_HARNESS_STACK.md)

## Programs

| Id | Prompt keywords | What Sol does |
|----|-----------------|---------------|
| `harness.calc_sum_gate` | calc / sum / add | Const seed → add via Call+`fn add` → Branch on `sum > 10` → say |
| `harness.semantic_ack` | ack / classify | classify + Branch + optional Park |
| `stack_top_a` | nested stack | A waits on B waits on C via `harness.invoke@1` |

Nested stack write-up: [`NESTED_HARNESS_STACK.md`](./NESTED_HARNESS_STACK.md)

| `harness.semantic_ack` | semantic / sentence / classify | Classify → Branch → clear ack **or** ask + Park + ack |

## Real LLM later

Production authoring is `aelio forge` (`FlowDrafter` / OpenAI host). This demo uses a deterministic mock so CI is closed; the **stored artifact is still App E Sol**.

## Not done yet (HS)

Wiring these contracts into Conductor / `HarnessProgramV1` play path is still Gate HS.
