# Exhaustive Sol harness system test matrix

**Status:** normative checklist for everything built so far in the Sol-harness slice  
**Authority for runtime proof:** `aelio-os/crates/aelio-kernel/tests/exhaustive_harness_system.rs`  
**Companion proofs:** `sol_harness_store_replay.rs`, `proper_sol_nested_stack.rs`, `sol_harness_lib` unit tests  
**Architecture context:** harness steps → Sol; nested harness wait; depth; pipelines; bags / context page stubs; store ↔ retrieve from DB  

**Rule:** every case ID below (`CASE-*`) has a matching `#[test]` named `case_*` in `exhaustive_harness_system.rs`. If a case is marked **Layer-B**, it is proven in `aelio-agent` harness tests (cited), not re-implemented in kernel.

```bash
cd aelio-os
cargo test -p aelio-kernel --test exhaustive_harness_system
cargo test -p aelio-kernel --test sol_harness_store_replay
cargo test -p aelio-kernel --test proper_sol_nested_stack
cargo test -p aelio-kernel sol_harness
cargo test -p aelio-agent harness_conductor
```

---

## 0. Architecture under test (what “working” means)

```text
Author / sugar name
    → App E Sol JSON (Seq / Call / Branch / Park / …)
    → compile + plan (aelio-kernel)
    → store in MemoryStore table `sol_harness_contracts` (Aelio DB trait)
    → load by id
    → execute Instance (bag in → bag out / Park)
    → ledger replay → bag_hash bit-identity
    → optional nested Call:
         sugar: harness.invoke@1 (runtime child id)
         proper: flow.stack_*@1 (registered-flow, acyclic graph)
```

**Bags:** Sol maps written via Const / Call `into` paths (`out`, `page`, `child`, `hits`, …).  
**Context page (demo):** `memory.search@1` → `context.attach@1` lands snippet at `page.note`.  
**Not yet Gate-HS:** live Conductor does not play these Sol contracts on the turn path (Layer-B still uses HarnessProgramV1). That gap is documented as `CASE-ARCH-01` (known open), not a silent fail.

---

## A. Library integrity & Sol compile (steps → Sol)

| Case | What | Expect |
|------|------|--------|
| **CASE-A01** | Seed library size | `sol_harness_library().len() >= 36` |
| **CASE-A02** | Every contract compiles + plans | `compile(program_json)` Ok for all 36 ids |
| **CASE-A03** | Catalog ids are unique | no duplicate `id` in library |
| **CASE-A04** | Every contract is App E shaped | root has `nid` + `op` after parse (via compile) |
| **CASE-A05** | Sugar stack trio present | `stack_leaf_c`, `stack_mid_b`, `stack_top_a` in library |
| **CASE-A06** | Proper Sol trio present | `proper_sol_stack_*` in library; mid/top Call pinned `flow.*@1`, not `harness.invoke@1` |

---

## B. Store / retrieve / idempotent DB put

| Case | What | Expect |
|------|------|--------|
| **CASE-B01** | `store_library` persists all | `list_contract_ids` length == library length |
| **CASE-B02** | Load each by id | every id returns Some; `kind == sol_harness` |
| **CASE-B03** | Round-trip program_json | loaded.program_json == source |
| **CASE-B04** | put_if_absent idempotent | second `store_library` → all `Existing` |
| **CASE-B05** | Table name | `CONTRACT_TABLE == "sol_harness_contracts"` |
| **CASE-B06** | Pull-out after store | load `memory_attach` / `stack_top_a` / `proper_sol_stack_top_a` and compile |

---

## C. Bags, context page, memory pipeline

| Case | What | Expect |
|------|------|--------|
| **CASE-C01** | `quick_reply` writes `out.text` | `"Quick reply: hello"` |
| **CASE-C02** | `memory_attach` search→attach→say | bag has `hits`, `page.note`, `out.text == "From memory: prior note"` |
| **CASE-C03** | `memory_search` only | `"Memory hit: prior note"` (no attach required) |
| **CASE-C04** | `full_reply` bag pull | `"Here is a fuller answer about: topic"` |
| **CASE-C05** | `apologize_closed` | fail-closed fixed text |
| **CASE-C06** | `understand_intent` | `intent.label` + `out.text` contains label |
| **CASE-C07** | Context page field survival | after memory_attach, `page.note == hits.snippet` |

---

## D. Control / compute Sol ops (harness bodies)

| Case | What | Expect |
|------|------|--------|
| **CASE-D01** | `calc_sum_gate` | `sum == 15`, `msg.text == "sum is large"` |
| **CASE-D02** | `calc_repeat_add` | `n == 3`, `"repeat-add done"` |
| **CASE-D03** | `calc_mul_div` | quotient path `"mul/div ok"` (or equivalent msg) |
| **CASE-D04** | `list_filter_keep` | `kept_count == 2`, `"filter done"` |
| **CASE-D05** | `string_contains_gate` help | `"I can help with that."` |
| **CASE-D06** | `string_contains_gate` else | `"Tell me how I can help."` |
| **CASE-D07** | `fallback_say` primary succeeds | `"primary line"` |
| **CASE-D08** | `guard_required_field` ready | `"ready field present"` |
| **CASE-D09** | `guard_required_field` missing | `"missing ready"` |
| **CASE-D10** | `budgeted_express` under budget | `"budgeted hello"` |
| **CASE-D11** | `once_external_stub` | Completes with `sent` present |
| **CASE-D12** | `detect_fresh_utterance` fresh | emits fresh signal path |
| **CASE-D13** | `greet_then_offer_help` help | `"I can help. What should we do first?"` |
| **CASE-D14** | `greet_then_offer_help` else | `"What do you need today?"` |

---

## E. Park / resume (conversational wait ≠ child wait)

| Case | What | Expect |
|------|------|--------|
| **CASE-E01** | `wait_for_user` Parks | `Parked` at nid `wait` after ask |
| **CASE-E02** | resume `wait_for_user` | Completes `"Thanks — continuing."` |
| **CASE-E03** | `semantic_ack` unclear Parks | park nid `wait`; resume → `"Thanks — noted."` |
| **CASE-E04** | `semantic_ack` clear completes | no park; clear message |
| **CASE-E05** | `clarify_slot` Parks | ask + Park |
| **CASE-E06** | `confirm_then_act` Park then yes | after resume with yes → tool stub acted |
| **CASE-E07** | `confirm_then_act` Park then no | `"Cancelled."` |

---

## F. Harness calls harness (sync wait) + depth

| Case | What | Expect |
|------|------|--------|
| **CASE-F01** | `parent_waits_on_child` | waits; child `sum==15`; parent out text |
| **CASE-F02** | Sugar nested A→B→C | `"top-A after mid-B after leaf-C done"` |
| **CASE-F03** | Proper Sol A→B→C | same text; Calls `flow.stack_*@1` |
| **CASE-F04** | Sugar vs proper agree | out text equal |
| **CASE-F05** | Unknown child id | `harness.invoke` Err; detail mentions unknown |
| **CASE-F06** | Parked child under invoke | Err; detail mentions parked |
| **CASE-F07** | Depth exceeded (self-invoke bomb) | Err `BudgetCalls` / depth exceeded |
| **CASE-F08** | Child bag passthrough | parent passes `utterance`; child quick_reply echoes it |
| **CASE-F09** | Proper flow graph acyclic | `validate_call_graph` Ok |
| **CASE-F10** | Proper flow cycle refused | leaf→top edge → Err |
| **CASE-F11** | Proper mid without flows fails | leaf registry alone → Err |
| **CASE-F12** | Mid-depth (2 levels) works | `stack_mid_b` alone completes |

---

## G. Pipelines & routers (composition)

| Case | What | Expect |
|------|------|--------|
| **CASE-G01** | `help_router` help path | `"From memory: prior note"` |
| **CASE-G02** | `help_router` else path | quick_reply echo |
| **CASE-G03** | `memory_then_full_reply` | fuller answer prefixed + memory text |
| **CASE-G04** | `greet_then_quick_reply` | completes (child bags present) |
| **CASE-G05** | `understand_then_memory` | completes with memory out |
| **CASE-G06** | `greet_memory_pipeline` | completes with memory out |
| **CASE-G07** | `fresh_then_greet` fresh | greet path |
| **CASE-G08** | `fresh_then_greet` else | quick_reply path |
| **CASE-G09** | `intent_then_calc` calc keywords | calc child sum path |
| **CASE-G10** | `intent_then_calc` else | understand path |
| **CASE-G11** | `calc_then_report_pipeline` | `"sum is large + mul/div ok"` |

---

## H. Replay / bag_hash / reuse

| Case | What | Expect |
|------|------|--------|
| **CASE-H01** | calc_sum_gate replay | replay hash == live bag_hash |
| **CASE-H02** | memory_attach replay | same |
| **CASE-H03** | sugar stack_top_a replay | same |
| **CASE-H04** | proper_sol_stack_top_a replay | same |
| **CASE-H05** | parent_waits_on_child replay | same |
| **CASE-H06** | Re-load from DB + re-run | same bag_hash as first run (reuse) |
| **CASE-H07** | bag_hash == value_hash(bag) | for calc_sum_gate |

---

## I. Architecture / known Gate-HS boundary

| Case | What | Expect |
|------|------|--------|
| **CASE-ARCH-01** | Documented: Conductor ≠ Sol play path yet | test asserts constant / doc flag only (not a runtime fail) |
| **CASE-ARCH-02** | Layer-B Conductor still routes starters | proven in `aelio-agent` `harness_conductor` (external) |
| **CASE-ARCH-03** | Layer-B wait retains context page notes | `wait_for_user_owns_next_message_until_resume` in agent |
| **CASE-ARCH-04** | Sol is durable law for harness bodies in this slice | every stored contract `kind=sol_harness` (CASE-B02) |

---

## Mapping: case → test function

All `CASE-*` (except ARCH-02/03 which cite agent tests) map to:

`exhaustive_harness_system.rs` → `case_a01_…`, `case_b01_…`, …  

ARCH-02/03 are executed via:

```bash
cargo test -p aelio-agent harness_conductor
```

---

## Pass criteria for “everything we have done so far”

1. This matrix file lists every loophole we care about in the Sol-harness system.  
2. `cargo test -p aelio-kernel --test exhaustive_harness_system` is **all green**.  
3. Companion Sol harness suites remain green.  
4. Agent Conductor suite green for Layer-B stack / pages (ARCH-02/03).  

**Open product work (not a test failure):** Gate HS — wire Conductor to load/play `sol_harness_contracts` on live turns.
