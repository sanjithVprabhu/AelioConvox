# Conductor / Harness OS — End-to-End Completion Checklist

**Authority:** `Blueprint/conductor-harness-os-production-plan.md`  
**Living tracker:** mark `[x]` only when tests + docs prove the step.  
**Last updated:** 2026-08-05

---

## Definition of done (full product)

- [x] Every event **can** enter Conductor (`/v2/events` + greets cut over on `/v1/turns`)
- [x] Conductor decisions stored/explainable (shadow + cutover traces; event admission)
- [x] Child harnesses nest + join with cancel + budgets (process tree; parallel dispatch later)
- [x] Executable steps are Sol or admitted Call (library + promote gate)
- [x] Vendor library installable from on-disk bundle (P0 families)
- [x] Parked trees hydrate waits/budgets from store
- [x] Effects intent-ledgered, Once-idempotent, cancel-safe, confirm workflow
- [ ] Old agent orchestration fully removed as authority (greets cut over; rest still dual)
- [x] Replay bag-hash on tree composition corpus (average)
- [x] Developer doc: author → test → promote → run (`docs/architecture/HARNESS_AUTHOR_PROMOTE_RUN.md`)

---

## Phase 0 — Lock & inventory

| # | Step | Status |
|---|------|--------|
| 0.1 | Vision + production plan accepted | [x] |
| 0.2 | Gap audit (kernel / runtime / agent planes) | [x] `docs/architecture/CONDUCTOR_HARNESS_GAP_REPORT.md` |
| 0.3 | Schemas: HarnessContract, Event, Action, Instance, Join, Result | [x] `aelio-kernel/src/os_contract.rs` |
| 0.4 | Semantic flags (dual IR, parallel-after-join, nested park) | [x] gap report HC-001..003 |
| 0.5 | Baseline kernel replay corpus still green | [x] `cargo test -p aelio-kernel` |

---

## Phase 1 — Library toolchain & Sol seeds

| # | Step | Status |
|---|------|--------|
| 1.1 | On-disk `aelio-os/library/` layout + manifest | [x] |
| 1.2 | P0 Call targets: `math.sum`, `math.divide`, `collection.count` | [x] |
| 1.3 | `workflow.average` sequential Sol + bag_hash replay | [x] |
| 1.4 | `compute::sum` list op | [x] |
| 1.5 | Expand P0 pure targets: logic, more math, text, time, id | [~] 2026-08-05: math/logic/text **complete** (20 targets — added `math.modulo/min/max`, `logic.not_equals/lt/lte/gte/and/or/not`). `time.*`/`id.*` intentionally still open: ledgered nondet, must not be `Pure` — guard test `nondeterministic_families_are_not_declared_pure` enforces this |
| 1.6 | Export seed + P0 workflows to library artifacts | [x] conductor, average, math, tools, memory, starters |
| 1.7 | Bundle builder: validate + hash + idempotent install | [x] `library_bundle::install_from_manifest` |
| 1.8 | CLI: `aelio library validate|install|list` | [x] |

---

## Phase 2 — Durable process tree

| # | Step | Status |
|---|------|--------|
| 2.1 | In-memory process tree: spawn / complete / fail / cancel | [x] `process_tree.rs` |
| 2.2 | `join=all` deterministic ordered result array | [x] |
| 2.3 | `join=any` / `all_settled` (then quorum) | [~] implemented in tree; more tests later |
| 2.4 | Parent/child instance ids + root linkage | [x] |
| 2.5 | Persist InstanceRecord / ChildJoin to store | [x] `process_store.rs` + hydrate restart test |
| 2.6 | Nested park/resume (process tree wait + auto-wake) | [x] |
| 2.6b | Persist wait predicates across restart | [x] `WAIT_TABLE` + hydrate |
| 2.7 | Cancel blocks post-cancel effect dispatch | [x] Instance.mark_cancelled + LiveBackend gate |
| 2.8 | Budget subdivision parent→child | [x] `InstanceBudgetV1` + charge/subdivide |
| 2.9 | Tree replay (parent + ordered child ledgers) | [x] `tree_replay.rs` bag_hash identity |
| 2.10 | Crash-injection (Once unknown outcome + cancel gate) | [x] `tests/crash_injection.rs` |

---

## Phase 3 — Orchestration + P0 programs

| # | Step | Status |
|---|------|--------|
| 3.1 | Syscalls: `harness.spawn`, `harness.await`, `harness.join_all` | [~] Rust API `ProcessTree` + `workflow_average_via_join`; registry Call wrap open |
| 3.2 | `harness.select` (rule then retrieval) | [~] `decide_deterministic` bootstrap |
| 3.3 | `workflow.average` via spawn+join (not only Seq Calls) | [x] |
| 3.4 | Starters as Sol in library: quick_reply, wait_for_user, memory.attach | [x] |
| 3.5 | `workflow.confirm_then_act` | [x] library + `run_confirm_then_act` |
| 3.6 | Reply/render + model structured validation programs | [~] express.say stubs; full model validate later |

---

## Phase 4 — Conductor cutover

| # | Step | Status |
|---|------|--------|
| 4.1 | `conductor.root` Sol — deterministic routes only | [x] program + library + tests |
| 4.2 | `NormalizedEventV1` admission path | [x] `event_admission.rs` dedupe + store |
| 4.3 | `POST /v2/events` (+ turn adapter body) | [x] agent-api route + `tests/v2_events.rs` |
| 4.4 | Shadow mode vs current `/v1/turns` | [x] `shadow.rs` + `Conductor.Shadow` step |
| 4.5 | Cut over pure greets/acks/farewells on `/v1/turns` | [x] expanded exact-token set |
| 4.5b | Tool/memory harness cutover on `/v1/turns` | [x] send otp / confirm / memory when installed |
| 4.6 | Canary + pin rollback | [ ] |
| 4.7 | Durable OS store (`AELIO_OS_STORE_PATH`) | [x] EmbeddedStore when env set |

---

## Phase 5 — Semantic routing, tools, memory

| # | Step | Status |
|---|------|--------|
| 5.1 | Retrieval + model disambiguation over top-K (validated) | [ ] deferred (rule select works) |
| 5.2 | Tool workflows as harnesses | [x] `tool.run_once`, `tool.send_otp` |
| 5.3 | Confirmed write workflow | [x] `workflow.confirm_then_act` |
| 5.4 | Memory attach harness | [x] `memory.attach` |
| 5.5 | Draft → sandbox → **admin promote** | [x] `promote.rs` |

---

## Phase 6 — Authoritative removal + harden

| # | Step | Status |
|---|------|--------|
| 6.1 | Remove agent dual IR as authority | [~] greets + tools + understand_intent + wait Sol cutover |
| 6.2 | Full P1 library families | [~] P0 vendor set shipped |
| 6.3 | Security review, load/soak, restore drill | [ ] ops later |
| 6.4 | OSS docs: author / test / promote / pin / run | [x] `HARNESS_AUTHOR_PROMOTE_RUN.md` |
| 6.5 | Production readiness sign-off | [ ] human/ops |

---

## Phase 7 — Learning & ecosystem (post)

| # | Step | Status |
|---|------|--------|
| 7.1 | Learn candidate → shadow → promote gates | [~] promote gates exist; learn later |
| 7.2 | Tenant authoring SDK/CLI | [ ] |
| 7.3 | Domain libraries + compatibility cert | [ ] |

---

## Immediate execution queue

### Done
1. ~~2.1–2.5 Process tree + durable persist/hydrate~~  
2. ~~3.3 average via join=all~~  
3. ~~4.1–4.4 conductor.root + events + shadow vs /v1/turns~~  
4. ~~1.7 library install/hash from manifest~~  
5. ~~1.5 partial stdlib expand~~  

### Done (also)
6–13. process tree park/cancel/budget/replay, greets cutover, durable OS store, math library  
14. ~~Tools: `tool.run_once`, `tool.send_otp`, `workflow.confirm_then_act`~~  
15. ~~Memory: `memory.attach`~~  
16. ~~Starters: `quick_reply`, `wait_for_user` on disk~~  
17. ~~Promote gate: draft → sandbox → admin_promote~~  
18. ~~Crash injection: Once unknown outcome + cancel gate~~  
19. ~~OSS author/promote/run doc~~  

### Done (latest)
20–26. tools + understand on turns/events  
27. ~~`full_reply` catch-all Sol — free-form no longer needs ProposePath~~  
28. ~~CLI: `aelio library validate|install|list`~~  
29. ~~13-harness vendor library validates clean~~  

### Remaining (honest residual)
1. Model-assisted top-K selection (rules + catch-all cover demos)  
2. Load/soak/security ops sign-off  
3. True parallel wave execution  
4. Optional: delete residual agent dual-IR code paths entirely  

**OS claim met for open-source demo:** install library → Conductor routes → greets/tools/intent/**full_reply catch-all** on turns and events → compose → promote → park/cancel/budget → tree replay → durable store → CLI.
