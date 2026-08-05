# Conductor / Harness OS — Gap Report (Phase 0)

**Date:** 2026-08-05  
**Authority:** `Blueprint/conductor-harness-os-production-plan.md`  
**Method:** Code audit + first production slice (schemas, library layout, `workflow.average`)

---

## Vision lock (product)

You want an **agent OS**, not a chat improviser:

| OS idea | Aelio |
|--------|--------|
| Instruction language | **Sol** (closed 17 ops + registered `Call`s); sugar only if it lowers to Sol |
| Program | Versioned harness artifact (typed I/O, effects, budgets) |
| Process | Harness **instance** (stack / tree) |
| Shell | **Conductor** harness — receives events, decides quick-reply / spawn / resume / exit |
| Memory | Bag + scoped pages; durable substrate = Aelio DB |
| Syscalls | Registered Call targets (math, tools, model, store…) |
| Growth | New tool paths become **promoted** harnesses after test/admin gate |

Average example: average harness takes a list → may spawn sum + count (series or parallel when join exists) → divide → typed output or structured error. Conductor only decides **which program** to run (or quick reply).

---

## Three planes today (risk)

| Plane | Location | Role today |
|-------|----------|------------|
| **A. Kernel Sol** | `aelio-kernel` | True ISA: plan/exec/ledger/replay; seed `sol_harness_lib` |
| **B. Runtime artifacts** | `aelio-runtime` | Durable Flow/Harness instances, sequential nested harness |
| **C. Agent Conductor** | `aelio-agent` | `HarnessProgramV1` JSON step IR + rule Conductor on `/agent/v1/turns` |

**Production convergence target:** A+B are execution truth; C becomes thin event ingress into a pinned Conductor Sol program. Dual IR (`HarnessProgramV1` vs Sol) is transitional, not the end state.

---

## Invariant matrix

| Plan invariant | Status | Notes |
|----------------|--------|-------|
| Harness-first; Conductor is pinned Sol | **partial** | Agent rule Conductor + step IR; kernel Sol lib not live turn path |
| All executable steps lower to Sol | **partial** | Kernel closed ops; agent step IR ≠ Sol; `HarnessBody` interpreted not lowered |
| Pinned Flow→Flow Call | **partial** | Proper-stack demos; not general production catalog |
| Nested park/resume across restart | **partial** | Runtime harness yes; kernel nested Call errors on Park |
| Child cont. addressable + parent-linked | **partial** | Runtime child ids; no full `InstanceRecordV1` tree yet |
| True parallel execution | **missing** | Wave analysis only; sequential exec |
| Budget inherit across instances | **partial** | Nested Budget in one Instance; fresh budgets for child instances |
| Cancel blocks post-cancel dispatch | **missing** | |
| Deterministic multi-child join | **missing** | Sequential `next_node` only |
| Replay nested/parallel trees | **partial** | Single Flow replay only |
| Stdlib `math.*` / `collection.*` Calls | **started** | P0: `math.sum@1`, `math.divide@1`, `collection.count@1` |
| `workflow.average` compound | **started** | Sequential pure Sol + replay (not SpawnMany yet) |
| Process schemas V1 | **started** | `os_contract.rs` |
| On-disk signed library layout | **started** | `aelio-os/library/` skeleton |

---

## What shipped in this engineering slice (2026-08-05)

1. **`HarnessContractV1`, `NormalizedEventV1`, `ConductorActionV1`, `InstanceRecordV1`, `ChildJoinV1`, `ResultEnvelopeV1`** — `aelio-kernel/src/os_contract.rs` with validate + content hash + unit tests.
2. **P0 pure stdlib Call targets** — `stdlib_targets.rs` wrapping `compute` (`sum` added for list sum).
3. **`workflow.average` Sol program** — seed in `sol_harness_lib` + on-disk `aelio-os/library/harnesses/workflow/average/1.0.0/`.
4. **Conformance tests** — `tests/workflow_average.rs`: success, empty→`Math.EmptyInput`, bag_hash replay, on-disk = seed.
5. **Library manifest** — `aelio-os/library/manifest.json|yaml` + README.

Honest non-claims:

- Not true parallel `SpawnMany [sum, count] join=all` yet.
- Conductor is not yet a live Sol root on `/agent/v1/turns`.
- No cancel-safe dispatch, no full process repository.

---

## Recommended next engineering order (from plan §18)

1. ~~Schemas V1~~ ✅  
2. ~~`workflow.average` sequential + replay~~ ✅  
3. Durable `InstanceRecordV1` persistence + parent/child links in runtime  
4. `join=all` for ordered children (even if children still run sequentially)  
5. Then parallelize pure independent children under wave legality  
6. `conductor.root` deterministic routes as Sol Flow  
7. Normalize `/agent/v1/turns` → event envelope adapter  
8. Shadow Conductor; cut over; retire dual agent IR  

---

## Open semantic flags

| ID | Topic | Recommendation |
|----|--------|----------------|
| HC-001 | Dual harness IR (agent step JSON vs Sol) | Prefer Sol-only for new work; lower or retire `HarnessProgramV1` after Conductor Sol cutover |
| HC-002 | Parallel before join | Ship sequential join semantics first; parallel is executor optimization |
| HC-003 | Kernel nested Park | Prefer runtime durable child instances over in-process nested `Instance` for production |

---

## Definition of “first vertical proof” (met)

- Stored Sol program for average  
- Namespaced pure Calls  
- Compile + plan + execute + bag_hash replay  
- On-disk library artifact matches seed  
- Explicit contracts for events/actions/instances ready for scheduler work  

**Not yet met:** full OS claim (Conductor shell, tree join, cancel, production cutover).
