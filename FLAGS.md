# FLAGS — implementation findings against `AELIO_DSL_MOTHER.md`

Findings, not failures. Format: `{id, section(s), what I found, options, recommendation}`. Provisional choices are marked `PROVISIONAL` in code comments. Blocking only if it gates correctness.

---

### F-001 — Workspace location (repo layout) — `§29`
**What:** The handoff says "use as repo `CLAUDE.md`" and names a fresh 8-crate Cargo workspace (`aelio-sol` … `aelio-cli`), but this monorepo already contains unrelated Rust (`Aelio DB/…`) and TS (`packages/`, `server/`). The mother doc does not pin a directory.
**Options:** (a) new top-level dir `aelio-os/` with its own workspace `Cargo.toml`; (b) place crates under repo-root `crates/`; (c) separate repo.
**Recommendation → (a) `aelio-os/`.** Clean separation from the legacy Aelio DB kernel, no workspace collision, easy to lift into its own repo later. `PROVISIONAL` — non-correctness; move is mechanical.

### F-002 — Store backend for P0 — `§24`, `§29`, Decision Log (Part VIII)
**What:** The doc commits Aelio DB as *the* store and says `aelio-store` "keeps a trait boundary for test doubles only." P0's §30 list needs Once/CAS and persistence, but P0's definition of done (login golden flow, replay bit-identity) is expressible against an in-memory double.
**Options:** (a) implement the `aelio-store` trait + in-memory double for all of P0, defer the Aelio DB binding to when P1 storage lands; (b) bind Aelio DB now.
**Recommendation → (a).** P0 exit criteria (§30) are all in-memory-satisfiable; the trait keeps Aelio DB a drop-in. `PROVISIONAL`.

### F-003 — `aelio-store` in the P0 dependency graph — `§29`
**What:** §29 lists `aelio-store` among the crates but the P0 build order (§30) touches storage only via Once/CAS and persistence. Dependency direction is "strictly downward"; the exact edges aren't drawn.
**Options:** (a) `aelio-kernel` depends on an `aelio-store` *trait* crate (store trait + in-memory double), inverting the concrete Aelio DB dep out of the kernel; (b) kernel owns persistence directly.
**Recommendation → (a) trait in `aelio-store`, kernel depends on the trait.** Matches "trait boundary for test doubles" and keeps the kernel Aelio DB-agnostic. `PROVISIONAL`.

### F-004 — Canonical float "shortest round-trip" formatting — `§4.3`
**What:** §4.3 mandates floats as "IEEE-754 f64, shortest round-trip form, always carrying a decimal point," `-0.0 → 0.0`, and cross-platform stability is a §27 property test. Rust's `{}`/`ryu` give shortest round-trip but not automatically "always a decimal point" (e.g. `2.0` prints `2`), and `-0.0` prints `-0`.
**Options:** (a) use `ryu` then post-process to guarantee a decimal point and normalize `-0.0`; (b) hand-roll a Grisu/Ryū-equivalent.
**Recommendation → (a).** `ryu` is the shortest-round-trip authority; a thin normalization layer enforces the decimal-point + `-0.0` rules. Covered by the int-vs-float and float-canonical conformance vectors. `PROVISIONAL` pending the cross-platform vector passing on CI.

### F-005 — Structural imprint over `var` resolution — `§4.1.3`
**What:** §4.1.3 says structural hashing resolves `var` values before hashing, and §4.1.4 says program-bearing bags (`fn`/`flow`) have *no* structural imprint. For `aelio-sol` in isolation there is no execution context to resolve a `var` path against.
**Options:** (a) `aelio-sol` computes structural imprints only over already-resolved, program-free bodies and returns a typed error (`ProgramBearing` / `UnresolvedVar`) otherwise, leaving `var` resolution to the kernel; (b) pull resolution into sol.
**Recommendation → (a).** Keeps `aelio-sol` zero-dep and pure; the kernel resolves `var` before asking sol to hash. `PROVISIONAL`.

### F-006 — Once across intentional Park — `§8.4`
**What:** Intent-without-result is fail-loud for crash recovery, but an intentional `Park` *inside* `Once` would leave status=`intent` until resume completes. Resume must not treat that as unknown-outcome.
**Options:** (a) park-aware Once status `parked`; (b) forbid Park inside Once at plan-time; (c) resume path skips re-claim (current) and only fails on *new* claim while intent open.
**Recommendation → (c) for P0.** Login golden has no Park inside Once. Plan-time forbid (b) is a clean P1 tightening. `PROVISIONAL`.

### F-007 — Annex location — handoff Phase 1
**What:** Handoff writes annexes to `docs/annexes/`; mother doc lives under `docs/claude_context/` with a root symlink `AELIO_DSL_MOTHER.md`.
**Recommendation → keep both.** Root symlink satisfies "repo root" references; annexes under `docs/annexes/`.

### F-008 — §9 compute catalog incomplete — `§9`, `F3`
**What:** `aelio-kernel/src/compute.rs` shipped only numeric/compare/logic/string/`is_type`/`blake3`, plus `pull`/`exists` in the Expr layer. The **locked §9 catalog** also mandates: structure `merge, drop, keep, path_copy`; list `count, append, first, last, slice, list_contains`; validate `matches_format`; and L0-C ledgered nondeterminism `now, uuid, random`. F3's completion check over-claimed ("≥3 vectors per op"). This is a P0 gap (handoff P0 step 4 = "Compute v0 §9/F3").
**Options:** (a) implement the pure ops in `compute::apply` now (merge/drop/keep/count/append/first/last/slice/list_contains), route `path_copy` as pure `(container, from, to)`; (b) leave list/structure to P1.
**Resolution → (a).** Pure ops added to `compute::apply` with vectors + unit tests. `path_copy` implemented as `(container, from, to)` (3-arg) — F3 said 2-arg "from,to" assuming an implicit bag edit; refined to explicit container for purity (compute stays pure, §A9.2). `matches_format` routed through the validator registry (§5.4) in the Expr layer alongside `exists`. `now/uuid/random` implemented as **L0-C ledgered nondeterminism** (§12.2 `nondet_value` INJECT entry on live, replayed from ledger per §12.3) — kept out of pure `compute::apply`.
**Decision Log:** 2026-07-27 · §9/F3 · path_copy signature refined 2-arg→3-arg (explicit container) · rationale: preserves compute purity (§A9.2); no locked invariant weakened.

### F-009 — §4.4 limits defined but not enforced — `§4.4`, `§4.2`
**What:** `aelio-sol`'s `Limits::check` (depth 32 / 1024 keys-per-map / 10k list len / 1 MiB canonical) was implemented and exported but **never called** by the kernel — bag writes could exceed the locked v0 caps unchecked. A locked invariant with no enforcement point.
**Resolution → enforce on every commit.** `Executor::write` and the `Const` whole-bag write now call `enforce_limits` → `Limits::default().check(bag)`, mapping any violation to `Budget.Size` (§11), so the bag invariant holds at all boundaries (persistence/Call/Park, §4.1). Tests in `limits.rs` (depth/keys/list caps reject; within-limits passes). No locked invariant weakened — this *adds* the missing enforcement. Deployer-tunable limits (§4.4 "tunable downward") left as future work; P0 uses defaults.

### F-010 — closed-schema not fully closed — `App E`, `§3`, `§4.4`
**What:** The parser rejected unknown **ops** but silently ignored unknown **fields** on a node, so a hallucinated/misspelled field (`arg` for `args`) parsed with the real field defaulting — violating "Unknown fields ⇒ plan-time reject" (App E) and the closed-schema threat-model guarantee (§3). Also, §8.2's "Const literal charged against §4.4 at plan time" was only enforced at runtime.
**Resolution → close both at plan time.** `instr::check_known_fields` validates every node's keys against a per-op allow-list (App E), rejecting extras as `Shape`. `plan::check_const_limits` charges every `Const` literal against §4.4 at compile, rejecting oversized literals as `Budget.Size`. Tests in `closed_schema.rs`. No locked invariant weakened — adds the missing rejections.

---
## Triage (2026-07-26)

| id | status |
|----|--------|
| F-001 workspace `aelio-os/` | **accepted** — built |
| F-002 store trait + memory for P0 | **accepted** — `aelio-store` |
| F-003 kernel→store trait | **accepted** |
| F-004 float canonical via ryu | **accepted** — sol tests green |
| F-005 structural imprint pre-resolved | **accepted** |
| F-006 Once/Park interaction | **deferred** P1 — login OK |
| F-007 annex/mother paths | **accepted** |

*This was the P0 triage snapshot. The later full-system findings below supersede its
“non-blocking” conclusion for an open-source production release.*

---

### F-011 — Two runtime implementations are not connected — `§22`, `§29`, `§31–§35`
**What:** The shipping TypeScript server under `server/` + `packages/core/` does not import, invoke,
or communicate with `aelio-os`. The Rust workspace now proves kernel semantics, but no production
request reaches it. Conversely, the existing TS turn pipeline predates the mother document and
cannot inherit its Planner, ledger, continuation, gate, or replay guarantees merely because the
Rust tests pass.

**Options:** (a) make the Rust runtime a local service and keep TypeScript as control plane/channel
adapters; (b) compile the kernel to a Node native/WASM module; (c) reimplement the locked semantics
in TypeScript.

**Recommendation → (a), BLOCKING open-source production claim.** A versioned local protocol keeps
the trusted kernel single-sourced, contains crashes, and lets the existing server/adapters migrate
incrementally. Native bindings have a smaller hop but a much larger build/distribution matrix;
duplicating the interpreter destroys the “one executor” replay guarantee.

**Status 2026-08-01 → RESOLVED.** `aelio-server` is the authoritative Rust process;
`aelio-runtime`, `aelio-agent-api`, and `aelio-db-api` are mounted together. Adaptive effects invoke
pinned, gated runtime proxies. The production TypeScript graph imports only `@aelio/core/edge`, and
`scripts/check-production-authority.mjs` recursively proves that no TypeScript decision rail is
reachable from server startup. Killing Rust fails turns closed; there is no edge fallback executor.

### F-012 — `aelio-store` has no Aelio DB production implementation — `§24`, `§29`, F10
**What:** `aelio-store` contains the trait and `MemoryStore` only. The durable continuation/WAL
logic is tested across reconstructed instances using the shared memory double, but not across an OS
process or against Aelio DB. The mother document explicitly commits Aelio DB as the production store.

**Recommendation → implement a concrete Aelio DB store after F-011 fixes the process boundary, then
run kill/restart, CAS-conflict, partial-WAL, and tenant-isolation integration tests. BLOCKING.**

**Status 2026-08-01 → RESOLVED.** `aelio-store::EmbeddedStore` is the production implementation over
the embedded Aelio database; runtime startup uses it and keeps `MemoryStore` as the test double.
Restart/CAS/recovery tests exist. Broader artifact repositories and crash-window matrices are Phase
1/9 work, not a missing store binding.

### F-013 — Synchronous target closures cannot enforce hanging-call deadlines — `§8.4`, `§10.1`
**What:** registry declarations require `DeadlineCompliant`, and active elapsed time is checked after
return, but a closure that never returns cannot be interrupted. This does not yet satisfy “Timeout
reaches inside hanging external calls.”

**Recommendation → async adapter contract with cancellation/deadline propagation in the Rust
runtime service. BLOCKING for untrusted/live adapters.** Reverse SDK calls should use Appendix H’s
authoritative server deadline rather than an in-process closure.

**Status 2026-08-01 → RESOLVED FOR PRODUCTION ADAPTERS.** The live reverse host is an authenticated
HTTP request with the kernel deadline applied to the request and a bounded client timeout. Closed
DSL execution is bounded and contains no arbitrary code. Trusted synchronous closures remain test
and embedded-adapter surfaces and cannot be forcibly preempted; this limitation is documented and
is not an untrusted production execution path.

### F-014 — Park inside Once remains an ambiguous crash state — `§8.4`
**What:** the current resume cursor skips a second claim in-process, but after restart the Once row
still says `intent`; continuation recovery can resume because it does not call `once_begin` again.
An operator or competing start using the same key sees unknown outcome. The state is safe
(fail-closed) but operationally indistinguishable from an actual crash during an effect.

**Recommendation → add a durable `parked` Once state tied to continuation hash, or reject Park under
Once in the Planner for v0. Prefer the Planner rejection until evidence requires the feature.**

**Status 2026-08-01 → RESOLVED.** The Planner rejects `Park` anywhere under `Once`, with regression
coverage. There is therefore no ambiguous persisted intent state in the admitted v0 language.

### F-015 — App I handler frame representation refinement — `App I`
**What:** durable Try-handler frames store the pinned handler index, while App I’s prose shows the
handler code-prefix. Because the envelope pins the exact flow revision, the index is deterministic
and sufficient, but the byte format differs from the illustrated normative field.

**Recommendation → store both `handler_index` and `handler_prefix` and cross-check them at decode;
this preserves O(1) resume and makes the envelope self-explanatory.**

**Status 2026-08-01 → RESOLVED.** Continuations store both fields, bounds-check the index and verify
the prefix against the pinned flow before handler resume.

### F-017 — Flow Forge v0 (prompt → draft → plan → store) — `§15/§18/§31`, cold path
**What:** Product need: enter a natural-language prompt, draft a closed App E flow, Planner-validate, optionally store. Mother doc allows LLM as author under a gate; full §16 artifact gate + pathway prototypes are P1. No end-to-end “forge” unit existed (ProposePath emits ability paths, not kernel trees).
**Options:** (a) minimal forge over a vendor Call catalog (`forge.say@1`) with mock + OpenAI drafters, HTTP `/v1/flows/forge` + CLI `aelio forge`; (b) wait for full gate.
**Resolution 2026-08-01 → (a), RATIFIED AS FORGE V0.** The catalog remains deliberately small. Model
output is only an authoring draft: closed parsing, the production Planner, sandbox and lifecycle gate
remain mandatory. Expanding the vendor catalog requires explicit admitted targets and evidence.

---

### F-018 — Prism query language vs §10.4 single-op AST — `§10.4`
**What:** Product needs one multimodal recall language (`from/match/where/select/limit/into`, RRF fusion, mandatory select+limit, no joins). Engine already runs `HybridQuery`+RRF. Mother §10.4 / `QueryAst` is one op per query. Prism is the natural façade; single-op becomes sugar.
**Options:** (a) ship Prism envelope in `aelio-query` + lowerer in `aelio-db-query`, keep `QueryAst` for simple Calls, amend §10.4 via Decision Log so flexible reads are Prism; (b) keep two languages forever.
**Resolution 2026-08-01 → (a), RATIFIED.** Mother Amendment #4 makes the implemented closed
multimodal Prism envelope canonical; `QueryAst` is compile-time sugar. Parser, physical-schema,
projection, vector-dimension, graph-budget, adversarial, database API, and server embedding paths
are implemented and tested. Ordering/cursors/general scans remain deliberately absent in v1.

---

### F-019 — Mint prompt factory (root axiom + Aelio DB shelf) — `§10.3`, App J
**What:** Product needs a command-shaped factory that mints versioned system-prompt artifacts (body + slots + output contract + description) from a human **root** axiom, stores them in Aelio DB, and recalls them via Prism. Mother App J already defines the coin shape; `aelio-prompt` was compose-only.
**Options:** (a) Mint inside Aelio (`aelio-prompt` artifact + root lock + `aelio-runtime` MintShelf over Aelio DB + CLI `aelio mint` / HTTP `/v1/mint`); (b) keep free-text prompts in the TS harness.
**Recommendation → (a) PROVISIONAL.** Root (`aelio.mint.root@1`) is never mintable. Sufficiency refusal is required (mock refuses blood-type-from-demographics). OpenAI drafter optional. Forge wiring to pin Mint coins is next.

**Status 2026-08-01 → RESOLVED.** `aelio-prompt::mint`, `MintShelf`, tenant-filtered Prism recall,
CLI/HTTP Mint, root immutability, closed `undeterminable` refusal, exact-contract exemplars,
mock/vendor drafters and prompt-class sandbox/lifecycle gating exist. Root and minted prompts lower
into the schema-versioned unified artifact registry; proposed prompts cannot be recalled or used.

---


---

### F-020 — Render Protocol E2E (aelio-render + @aelio/chat-sdk) — `AELIO_RENDER_PROTOCOL`
**What:** Product needs typed RenderFrame down / EventFrame up between Rust agent Express and the web widget. Previously only plain `Utterance.text` crossed the seam.
**Options:** (a) ship closed-kind `aelio-render` + TS `@aelio/chat-sdk`, author frames in Rust before send, TS validates + renders with mandatory fallbacks; (b) invent frames from free text in TS.
**Resolution 2026-08-01 → (a), RATIFIED V1.** Hello/Welcome negotiates closed kinds and bounded
limits; Rust `Express` emits the authoritative frame; the widget validates/renders required
fallbacks; WhatsApp deliberately uses the flattened representation. The public phase suite asserts
Render frames. Additional element kinds are versioned extensions, not missing v1 authority.

**Residual (2026-08-02):** Protocol F5 full frame ledgering for exact resume reconstruction remains
stronger than “widget received a valid text@1 frame.” Track under Gate G3-16 of
`docs/operations/PRODUCTION_READINESS_CHECKLIST.md` — either implement reaction-ledgered frames or
keep this residual explicit in release notes. `PROVISIONAL` for open-source RC until chosen.

---

### F-021 — Invented bound inventory during unified cutover — `§4.4`, `§18`, `§21`, `§33`
**What:** Unified cutover introduced several numeric bounds not yet cited from mother §33/App tables.
Known sites after Mother §18 pathway-cap fix and adaptive BLAKE3 migration:
- adaptive envelope size/depth/score/projected-input caps (`aelio-agent/src/adaptive.rs`) — candidate
  cap 8 aligned with §18; other byte/depth caps need §4.4/§33 cites or stay provisional;
- builder RRF `k` / ranking constants (`aelio-runtime/src/builder.rs`);
- learning/gate thresholds such as 20 distinct observations / 0.95 agreement
  (`aelio-runtime/src/learning.rs`, `gate.rs`) — Appendix K / steward thresholds where applicable.
**Options:** (a) cite or move each into App/§33 constants now; (b) leave `PROVISIONAL` with this flag
until the constant audit pass (Gate G1-3) closes them one-by-one.
**Recommendation → (b) for this remediation slice**, with checklist Gate G1-3 owning the inventory.
Pathway prototype cap is **not** provisional: clamped to **8** per Mother §18.
Adaptive decision identity is **not** provisional: Mother §4.3 Sol + BLAKE3.

### F-022 — Semantic-only flows vs install readiness — authored-flow lowering
**What:** Ops doc previously said any flow without lowering keeps readiness degraded. That made
journey/guidance flows block "OS install complete" even when no executable lowering was declared.
**Options:** (a) keep every unpinned flow blocking ready; (b) only lowering-declared flows are
install debt — semantic-only guidance does not block ready; turns refuse operate while executable
pending > 0; matching skips semantic-only without fail-closed.
**Recommendation → (b).** Author control: ship `aelio.lowering` to opt a flow into the install.
`PROVISIONAL` relative to older "always degrade" wording; aligned with install-then-operate.

### F-023 — Session harness stack beside Mother session sense — Conductor vision
**What:** Product vision requires a multi-frame harness stack + durable per-harness context pages
(`docs/architecture/HARNESS_CONDUCTOR_VISION.md`). Mother session/`sense` shapes are flatter.
**Options:** (a) overload FlowInstance as fake stack; (b) new `HarnessSessions` logical table +
control plane in `TurnRuntime::run` before waiting-child resume.
**Recommendation → (b).** Implemented as `aelio-agent/src/harness` + `LogicalTable::HarnessSessions`.
`PROVISIONAL` pending live Conductor preinstall proof; does not weaken pin admission.

### F-024 — Conductor narrows the procedure-learning surface — greetings no longer promote
**What:** With Conductor default-on, `TurnRuntime::run` returns at `LookupTier::Tier3` for every
starter selection except `Escalate`. `learn_from_turn` sits inside the ProposePath branch below that
return, so Conductor-handled turns (greetings, quick replies, clarify) no longer feed
`observe_and_promote`. Found only after Gate H0-1 restored compilation of `tests/durable_workers.rs`,
which had not built since the harness slice landed; 2 tests were failing silently behind the build
break.
**Impact measured, not assumed:** the learning ladder itself is intact. A cold-path utterance
(`"list jobs"`) still runs `Tier2 x3 -> Tier0` with a durable promoted procedure. Only utterances the
Conductor answers directly stop learning.
**Options:** (a) accept the narrowing — the deterministic harness is already the fast path, so a
learned procedure adds nothing for greetings; (b) observe Conductor-played harnesses too, synthesising
an `AbilityPath` per harness so promotion continues.
**Recommendation -> (a).** For a greeting the Conductor reaches 0 `llm_calls` on turn **one**; the old
loop reached the same cost only after 3 observations promoted it. (b) would pollute the procedure
registry with entries that can never beat the harness they duplicate. Cold-path learning — the surface
Phase 7 create-harness actually builds on — is unchanged.
**Test consequence:** `repeated_success_promotes_durably_and_next_turn_is_tier_zero` and
`durable_promotion_and_tier_one_use_the_same_configured_embedding_space` retargeted from `"hi"` to a
cold-path utterance (their subjects are promotion mechanics and embedding-space identity; the greeting
was an incidental vehicle). The greeting guarantee it used to carry — no manufactured model-token cost
— is preserved as a stronger assertion in the new
`conductor_answers_greetings_without_cold_path_or_model_cost`: zero model calls, no ProposePath, and no
learned procedure at all.
`PROVISIONAL` — revisit if Conductor-owned turns ever need warm-tier acceleration.

### F-025 — Path B (Sol-only bodies) resolves LAYER_BREAKDOWN open Path A vs B
**What:** `LAYER_BREAKDOWN.md` §P1 left Path A (sugar + lowering + dual runtime) vs Path B (Sol-only)
open. Execution plan §2 chooses **B**: sugar may exist as authoring input but is compiled to Sol at
admission; only Sol is stored, hashed, and executed. `HarnessStepV1` / `play_harness_program` are
deleted in Phase 5, not maintained forever.
**Rationale:** dual runtimes fork behavior silently (the exact class of bug that produced "stored
programs are persisted" while the live catalog held `{}`). Authoring ergonomics is a tooling problem
above storage.
**Identity:** harness identity is Mother §4.3 canonical Sol + BLAKE3 (`aelio_sol::value_hash`), never
ad-hoc SHA-256 over serde_json (checklist G1-2 / plan O2).
**Status:** decided; Phase 1 contract model implements sealed identity + admission. Phase 2 freezes
the Call ISA next. No Mother amendment required for Path B itself.

### F-026 — `tool.invoke@1` host proxy vs Mother §12.4 intent/dispatch/result
**What:** Phase 2 freezes `tool.invoke@1` as the sole tool Call id (replacing ad-hoc `tool.act_stub@1`
for new work). Mother §12.4 requires effectful Calls to carry ledgered intent→dispatch→result and
Once-wrap for idempotency. A Sol program body must never open a network socket or SDK channel
directly — only the runtime-owned proxy may.
**Options:** (a) kernel registers a deterministic stub that echos `{tool_id,args,corr}` for tests,
and production hosts **re-register** the same id with a reverse-channel adapter that ledgers
intent/result and refuses bare re-execution without Once; (b) amend Mother to allow body-local
tool dispatch (rejected — weakens §12.4); (c) invent a new op class outside Call (rejected — Call
is the T3 surface).
**Recommendation → (a).** Implemented in `aelio-kernel/src/call_isa.rs`: default stub is External +
deadline-compliant + policy-tagged; host swap is the production path. Harness authors must wrap
`tool.invoke@1` in `Once` when the tool is effectful. `tool.act_stub@1` remains registered for
seed-library back-compat only.
**Open:** whether host re-registration needs a dedicated `Registry::replace_host_target` API vs
building the registry only once at boot with the real adapter. Prefer boot-time injection for now.
`PROVISIONAL` until agent-host wires the reverse channel.

### F-027 — Harness v2 TaskGraph contract above kernel — `§4`, `§8`, Harness v2 plan
**What:** Harness v2 introduces `TaskNode` / `TaskGraph` / `EvalVerdict` as orchestration-layer
contracts (goal, rationale, strategy_hint, per-node budgets, refinement loop). Mother §4 covers
Sol bags and imprints; it does not name TaskGraph explicitly. Multi-agent decomposition is a
**planning/orchestration** artifact that lowers to Sol op trees + existing Call targets — not a
new kernel op class.
**Options:** (a) treat TaskGraph as agent-layer JSON contracts in `aelio-sol::task_graph` with
canonical hashing for promotion evidence; (b) amend Mother §4 to normatively define TaskGraph.
**Recommendation → (a).** Each TaskNode lowers to Sol before kernel execution; ledger/replay
invariants remain on kernel turns. Promotion stores situation key → TaskGraph hash or lowered Sol
pin. `PROVISIONAL` until Phase 7 promotion vectors pass.
**Orchestrate insertion:** `aelio-agent/src/blocks/turn.rs` — after tier lookup on cold path
(Tier3 escalate → ProposePath branch), before flat `execute_tier_path`. Conductor starters and
warm pin matches bypass Orchestrate. See `docs/development/Aelio_harness_v2_multiagent_architecture.md`.

### F-028 — Ephemeral runtime sub-agents — Harness v2, std tool catalog
**What:** When orchestration cannot match a pinned workflow or registered `std.*` tool, the system
needs a bounded fallback that mints a short-lived sub-agent (verified pipeline of pure `std.*`
steps), executes it once, and destroys it — analogous to attach-doc-then-discard patterns in other
agent systems. Mother doc does not define ephemeral tools; this is agent-layer only.
**Options:** (a) `std.ephemeral.adapt` meta-tool + `EphemeralScope` RAII lifecycle in
`orchestration/ephemeral.rs`; (b) fall through to cold ProposePath only; (c) LLM-synthesized
arbitrary code (rejected — non-deterministic, unauditable).
**Recommendation → (a).** Heuristic synthesizer maps document/text/count goals to std pipelines;
`EphemeralScope` tracks created/destroyed IDs per turn for trace. LLM-driven synthesis deferred to
Phase E. Ephemeral pipelines are **not** promoted to warm path unless separately observed.
**Routing (F-028b, amended):** `tool_router.rs` enforces pinned workflow → std → ephemeral →
fallthrough. Orchestration **never selects a tenant tool from utterance keywords.** The original
`match_client_tool` scored capability-tag segments, so `auth.otp.send` matched the word "send" in
ordinary prose and reached the host with every gate skipped. It is deleted. Tenant tool selection
belongs to ProposePath, where the proposal is typechecked against declared abilities.
A tenant tool reaches `execute_task_graph` only when a verified TaskGraph node names it in
`strategy_hint`, and it is then invoked through `blocks/tool_call.rs::run_tool_call_block` —
policy, `resolve_all` parameter binding, derived per-call `idem_key`, error-envelope
classification, and response redaction all apply. Unbound required params return
`GraphExecution::NeedUser` rather than calling with a missing argument. `std.*` and ephemeral
tools execute in-process but still pass `require_allow` at `ReadOnly` risk, so a tenant can deny
specific first-party tools. `PROVISIONAL`.

### F-029 — The `std.` tool namespace is reserved — Harness v2, `§tenant catalog`
**What:** Nothing but the id prefix distinguishes a first-party tool the runtime executes locally
from one it dispatches over the tenant's SDK — there is no `provider` field on `ToolSpec`. A
tenant registering `std.math.average` would therefore both shadow the first-party tool and have
their "tool" silently executed in-process.
**Options:** (a) reserve the prefix and reject tenant registrations that use it; (b) add a
`provider` field to `ToolSpec` and key routing on that; (c) namespace tenant tools instead.
**Recommendation → (a) now, (b) later.** `Registry::register_tenant_tool` rejects the `std.`
prefix with `ReasonCode::Validation`; `register_tool` stays infallible for first-party boot
registration. (b) is the cleaner long-term model but changes a serialized tenant contract, so it
is deferred rather than taken silently. `PROVISIONAL`.

### F-031 — Ability retrieval before ProposePath — Harness v2, `§ProposePath`
**What:** ProposePath serialized *every* declared ability, with tools and params, into one prompt.
Registering the 58 first-party `std.*` tools took the demo tenant from 9 abilities to 67, so every
cold prompt now carried the whole utility catalog. This scales badly twice over: context limits,
and selection accuracy degrading well before the limit is hit.
**Options:** (a) embedding retrieval to shortlist candidates before prompting; (b) exclude `std.*`
from ProposePath entirely; (c) leave it and cap tenant catalog size.
**Recommendation → (a).** `abilities/retrieval.rs` ranks abilities against the utterance using the
existing `Embedder`, following the `term_resolve` / `classify_intent` precedent: deterministic
token coverage, with embedding cosine as a semantic bridge that is inert under the offline
bag-of-hash embedder and live under a real model.

Three properties make this safe to put in front of a gate:

1. **Retrieval nominates, it never decides.** ProposePath still selects and the selection is still
   typechecked; the tool-call gate still authorizes. Because `propose_path_with_provider` uses the
   id list as its allow-list, shortlisting can only *narrow* what the model may pick — a poor
   shortlist costs a failed-closed proposal, never a wrong effect.
2. **Recall over precision.** A near-tie is retained rather than resolved — the inverse of
   `term_resolve`, which escalates a thin margin to the user, because there the score selects and
   here it only nominates. A group that scores flat is passed through whole. Tie expansion is
   capped at 2× budget so a uniformly-scoring group cannot defeat the budget entirely; ordering is
   score-desc then id-asc, so that cut is deterministic.
3. **Tenant abilities are budgeted separately from `std.*`.** Observed during implementation:
   for "list jobs" the `std.data.*` descriptors matched lexically and evicted every tenant
   capability from the shortlist. Partitioning fixes it — the tenant/engine set has its own
   reserve and the utility catalog is sampled within its own budget. Demo tenant goes 67 → 17
   with all three tenant abilities and all six engine abilities retained.

Catalogs at or below `always_inline_below` (24) pass through untouched, so small tenants see
today's behavior exactly. `PROVISIONAL` — thresholds are unvalidated against a real large catalog.

**Not addressed:** whether `std.*` belongs in the ProposePath candidate set at all. Option (b) is
arguably cleaner — those are first-party pure ops, not tenant capability composition — but it
changes what paths can be proposed, so it is deferred rather than taken silently.

### F-030 — Orchestration ledger hashing must be content-stable — `§4.3`
**What:** `wavefront::hash_args` used `std::collections::hash_map::DefaultHasher` behind a helper
misleadingly named `md5_hash`. `DefaultHasher`'s output is stable for neither Rust versions nor
platforms, so a ledger `args_hash` could not survive replay. Separately, `std.ephemeral.adapt`
embedded a wall-clock-nanosecond `_ephemeral_id` in its node output, which landed in
`ExecutorState.ledger[].result` and broke replay bit-identity outright.
**Resolution:** `hash_args` is BLAKE3 over key-sorted canonical JSON via `aelio_sol::blake3_hex`.
Ephemeral pipeline ids are derived from the scope sequence plus a BLAKE3 digest of the goal, never
the clock, and stay in `EphemeralScope` — they no longer enter node output. Covered by
`repeat_execution_produces_an_identical_ledger`. Not provisional; this restores a locked §4.3
invariant.

---

### F-032 — `docs/updates/HARNESS_KERNEL_V4.md` doc set vs. `aelio-os` — cross-cutting
**What:** Three untracked documents (`HARNESS_KERNEL_V4.md`, `CONVERGENCE_MATRIX.md`,
`M1_STATUS.md`) describe a system with different terminology (`starlark-rust`, `Astrolobe`,
`redb`, a `harness-core` crate) from the one actually implemented here. `M1_STATUS.md` claimed a
`harness-core/` crate tree with 34, then 54, passing tests; a repo-wide and whole-filesystem
search found this tree **does not exist anywhere on this machine**. Directed to treat the two
systems as the same product idea under different names, map terminology, and fix real
divergences — four parallel code audits checked ~24 specific HARNESS_KERNEL_V4 claims against the
real code.

**Terminology map (HKv4 → real `aelio-os`):**

| HKv4 | Real system |
|---|---|
| `harness-core` (numeric/canonical/agg/versioning) | `aelio-sol` (`value.rs`, `canonical.rs`, `hash.rs`, `limits.rs`) |
| Astrolobe (control-plane vector+graph+text+columnar) | Aelio DB (`aelio-db-graph`, `aelio-db-index`, `aelio-db-text`, `aelio-db-format`, `aelio-db-query`) |
| `redb` data-plane store | `aelio-store` (`EmbeddedStore` over the same Aelio DB engine) |
| Skeleton + typed holes + signature | Edge-scoped converters (`aelio-convert`, `edge_id`) + literal `AbilityPath` procedures (`aelio-agent/abilities/registry.rs`) — no hole abstraction |
| Phase 0–3 matching + residue check | `LookupTier` Tier0–3 ladder (`aelio-agent/abilities/learn.rs`) — **no residue-equivalent existed** (fixed below) |
| Reply / Compute / Clarify triage | `StarterHarness` (QuickReply / Escalate / UnderstandIntent / WaitForUser) in `harness/conductor.rs` |
| Per-value taint bits | Args-only least-privilege projection into `Call`/`Map` boundaries (mother §4.2.4, §6.3, §10.3.1) — different mechanism, same goal |
| `SystemVersion` / `Verified<T>` / `impl_hash` | No equivalent; closest is per-Call-target `version` pinning (mother §10.1) |
| Ledger `LedgerEntry` (path/skeleton_sig/join_path_hash/…) | `aelio-kernel::ledger::Entry` (`seq, turn_id, nid, kind, category, payload, payload_hash, prev`) — different, mother-defined shape (App G) |
| Dual volume/evidence promotion routes | Single reach-tiered gate, `aelio-convert/src/gate.rs` (Structural→Shadow→Canary→Promoted, distinct-input thresholds) |

**Verdict, by area (full detail in the four audit transcripts this entry summarizes):**
- **Aligned already:** canonical float/map hashing (BLAKE3, ryu, `-0.0→0.0`, sorted keys,
  `canonical.rs`); the unified vector+graph+text+columnar store (Aelio DB); fail-closed matching
  (`learn.rs` Tier1 never executes a below-threshold nearest neighbour); Once/CAS idempotency
  (§12.4); args-only least-privilege boundary.
- **No equivalent, by design — accepted as a different valid design, not a defect:** `Decimal`
  numeric tower + null-explicit aggregate ops (real type system is mother §5's
  `null|bool|int|float|str|list|map`, no `Decimal`); skeleton/hole template reuse (real reuse is
  edge-scoped converters + literal procedure paths); per-value taint bits (args-only projection
  instead); `SystemVersion`/`Verified<T>`/`impl_hash`; privacy modes (Private/Assisted/Isolated) +
  transmission allow-list; `redb`-shaped data-plane tables; HKv4's `LedgerEntry` field shape;
  multi-replica ledger lease/segments (mother §33: single-instance v0, tenant-sharded scale-out);
  HKv4's named two-route (volume/evidence) promotion ladder.
- **Genuinely missing, fixed by this entry (see Resolution):** a residue-style "was every stated
  constraint actually consumed" check; a numeric-fabrication guard on the Reply/QuickReply path;
  synchronous contract-invalidation blast-radius reporting at tool registration;
  determinism-hygiene guardrails (`HashMap` ban, cross-arch CI) scoped to the hashing-critical
  crate.
- **`M1_STATUS.md` pass-2 findings N12–N15, checked against real code:**
  - **N15** (replay past journal end must hard-refuse, never dispatch) — **already true**:
    `ReplayBackend::pop` in `aelio-kernel/src/driver.rs:912-916` returns
    `Err(ReasonCode::Internal, .., "replay ran past the ledger")`.
  - **N14** (Budget needs CAS, not `fetch_sub`, for concurrent `map_tool`) — **not applicable**:
    real `Map` walks elements sequentially (`aelio-kernel/src/exec.rs:761-866`); no concurrent
    race exists in v0. Re-check only if `Map` becomes parallel.
  - **N12/N13** (taint implicit-flow, map-key taint laundering) — **not applicable**: no
    taint-bit system exists to have this class of bug in.

**Resolution → accept the "genuinely missing" list above as real gaps and close them:**
1. Residue check: heuristic constraint-fragment extraction + consumption check, wired into
   `blocks/turn.rs` after `ProposePath` typecheck, before execution.
2. Reply numeric guard: scan synthesized `QuickReply` text for digits/currency/`%`; on a hit,
   force re-triage to `Escalate` instead of serving the reply.
3. Contract-invalidation blast radius: `Registry::register_tool`/`register_tenant_tool` now call
   `invalidate_tool` synchronously on a content-changing re-registration and return the suspended
   procedure ids.
4. Determinism hygiene: `aelio-sol/clippy.toml` bans `HashMap`/`HashSet` (crate-scoped, since
   `aelio-sol` is already `BTreeMap`-only and other crates use `HashMap` legitimately elsewhere);
   CI gains a native `ubuntu-24.04-arm` job running `aelio-sol`'s own test suite.

Everything in the "no equivalent, by design" list is `PROVISIONAL` only in the sense that a future
`AELIO_DSL_MOTHER.md` amendment could adopt one of HKv4's mechanisms instead — none of them is a
defect in the current locked spec, so none is queued as follow-up work.

---

### F-033 — Framework migration: HKv4 + Starlark replaces mother-doc authority — cross-cutting
**What:** Product direction shifts from DSL interpretation (`AELIO_DSL_MOTHER.md` Planner/Executor)
to **LLM-generated Starlark modules** executed under a deterministic harness (HKv4 /
`IMPLEMENTATION_VERIFICATION.md`). The mother doc remains in-repo for historical P0 work but is
**no longer implementation authority** for new features.

**Decision (2026-08-07):**
1. **North star:** `docs/updates/HARNESS_KERNEL_V4.md` + `IMPLEMENTATION_VERIFICATION.md`.
2. **Substrate crate:** `aelio-os/crates/harness-core/` — numeric, agg, taint, versioning, exec
   (Phase 2; stubs landed in F-033, implementation follows Starlark wiring order).
3. **Bridge:** existing TaskGraph orchestration + `aelio-sol` canonical hashing stay until
   Starlark eval is locked (§8 dialect flags, taint wrapper, budget pool).
4. **Phase 1 fixes (this entry):** promotion σ alignment, TaskGraph/wavefront hash paths off
   serde_json, NeedUser suspension, determinism tests, effectful detection via registry.

**Rationale:** LLMs produce better imperative code than rigid DSL hole-filling; the harness
invariants (determinism, taint, budget, versioning) must be settled *before* the embedding,
not bolted on after — Section 8 is `[KILL if wired]` without them.

**Status:** Phase 1 fixes applied; Phase 2 work order in
`docs/updates/PHASE_2_INSTRUCTIONS.md`. **Correction:** full `harness-core` (2,111 lines, 54
tests) exists in external deliverables — Phase 1 incorrectly scaffolded stubs; Task 2 is
**port, not reimplement**. `AELIO_DSL_MOTHER.md` frozen — amendments only via explicit FLAGS entries.

---

### F-034 — Starlark codegen: text emission + AST authority (hybrid) — §8.3
**What:** Phase 1 report ratified "LLM-generated Starlark text" over HKv4 §8.3 AST-as-JSON without
flagging the cost: code-emission parse failures (~2.4%) and >20pt success drop. Pure text hashing
also fragments the content-addressed store (whitespace/comment variants → different hashes).

**Decision (2026-08-07) — hybrid with one non-negotiable rule:**
1. Model **may** emit Starlark **text** (comfort zone for hosted APIs without grammar-constrained decoding).
2. Text is **immediately parsed**; from that point the **AST is the only authoritative artifact**:
   - `harness_hash` = BLAKE3(canonical re-render(parse(text))), **never** raw emitted text.
   - Skeleton extraction (§6.3) runs on parsed AST.
   - Verification rendering for Phase 3 / user is rendered from AST by the same renderer.
3. Parse failure → capped repair loop (max 3) with structured diagnostics → Clarify. No unbounded retry.
4. **Property test required:** `render(parse(text))` re-parses to identical AST for every authored harness.

**Rejected:** Pure text with text-hashing — accepted only if explicitly chosen; would fragment cache.

**Status:** Decision locked; implementation in Phase 2 Task 7 (after substrate Tasks 2–6).

---

### F-035 — Fail-closed gating stubs — orchestration eval (Task 3)
**What:** `orchestration/executor.rs` semantic eval hook returned unconditional `pass: true`; exhausted
refinement loop returned `NodeStep::Done` even when verdict failed — downstream believed verification ran.

**Resolution → shape-only gate + fail-closed on exhaustion:**
- Semantic hook removed; only `evaluate_shape` runs until semantic gate is implemented.
- Shape failure after refinement budget → hard error, not silent Done.
- `GraphSuspension` types: **pending** wire into NeedUser resume or delete (Task 3.3).

**Gates currently fail-closed-pending-implementation:**
| Gate | Location | Status |
|------|----------|--------|
| Semantic refinement | `executor.rs` | Removed fake pass; shape only |
| GraphSuspension resume | `suspension.rs`, `orchestrate.rs`, `world.rs` | Wired: NeedUser saves snapshot; next turn resumes via `try_resume_orchestrated_graph` |
| Per-step replay divergence | `harness-core/exec.rs`, `aelio-kernel/tests/replay_steps.rs` | `verify_replay` reports first diverging seq; kernel underrun typed `Journal.Underrun` |

**Status:** Sessions A–C complete (2026-08-07). **Session D0 blocks D+** — see F-036.

### F-036 — harness-core reimplementation vs 54-test deliverable (Session D0)
**What:** Phase 2 Session A reimplemented `harness-core` from HKv4 spec (40 tests), not ported from the original 54-test bundle. Fourteen behaviours may be untested or implemented with contested defaults (division scale, negative banker's ties, agg float-order sentinel, CAS vs fetch_sub, three null policies, implicit flow, etc.).

**Resolution:** [`docs/updates/PHASE_2D_INSTRUCTIONS.md`](docs/updates/PHASE_2D_INSTRUCTIONS.md) Part 0 — test-by-test reconciliation by name, API-shape checks (Verified&lt;T&gt; privacy, SystemVersion exhaustive destructuring, CAS drain), hash provenance freeze. Re-pin hashes once at D0 exit if serialisation changes; then `SystemVersion.serialiser` bump only.

**Status (2026-08-07):** D0 complete — 65 harness-core tests pass in debug and release. The reconciliation added the three-null-policy aggregate golden case, negative half-even and decimal-scale coverage, conservative container/PC taint propagation, CAS-backed frame budgets, journal underrun/stub semantics, exhaustive version-field perturbation, effect-set bounds, and determinism construction-path/repetition fixtures. Canonical serialisation did not change; pinned hashes remain `a66bcd6a…` and `0140b77a…`. `check-canonical-writer.sh` passes. D1–D6 may proceed in their declared dependency order.

### F-037 — Bridge warm-path time-window cache audit (Part 5c urgent)
**What:** If the bridge warm path caches resolved calendar windows (e.g. "last 7 days" computed once at promotion), reuse serves stale answers on every subsequent hit until the cache expires — independent of the D4 time module.

**Audit (2026-08-07):** Grep on `aelio-agent/src/` for `days_ago`, `LastNDays`, calendar windows intersecting `cache|promot|warm|lookup|situation` — **no calendar-window cache found** on the warm lookup path. `SituationKey.last_seen_bucket` (visit recency) is in σ; it is not a calendar validity bucket.

**Options:** (a) add `validity_bucket` to σ when D4 time module lands; (b) hotfix now if grep finds resolved dates in promotion store.

**Recommendation → (a) unless grep finds a hit.** Re-run the Part 5c grep before D4 and before enabling matching on real traffic; record result in this flag.

**Status (D4 re-audit, 2026-08-07):** Clear. Re-ran the prescribed search across
`aelio-agent/src/`; its only cache-adjacent time hit is a durable exploration-rate
window, not a resolved user calendar range or warm procedure lookup. No
`SituationKey.validity_bucket` was added: changing σ without a time-relative cached
plan would fragment warm matches without protecting a stale answer. Re-audit before
matching is enabled against real traffic.

### F-038 — Month-to-date cache validity granularity — `HARNESS_KERNEL_V4 §2.7`
**What:** `TemporalBinding::MonthToDate` has an exact journaled-now upper bound, while
the specification leaves the caller-selected `granularity` open. A cache keyed only by
a Month bucket would therefore reuse a range with an old upper bound during the same
month.
**Options:** (a) require `Hour` or finer validity for `MonthToDate`; (b) redefine the
range as the full current calendar month; (c) remove it from cacheable relative forms.
**Recommendation → (a).** The D4 core records both the exact resolved interval and
the caller-provided granularity but has no time-relative warm-plan cache to enforce
this contract yet. The future cache admission API must reject `MonthToDate` unless its
validity bucket advances no less often than the chosen upper-bound semantics.

### F-039 — Gate J observation evidence is not yet collectable — `AELIO_MASTER_PLAN Gate J`, `PHASE_2_ACCEPTANCE_SUITE J1–J3`
**What:** Gate J requires one week of production traffic, daily raw cold/warm/distinct-key/
executions-per-key observations, and 20 manually selected warm hits re-run cold. The current
`aelio-agent::reuse_metrics` collector is a process-local `OnceLock<Mutex<_>>`; it is reset at
restart, has no dated snapshots or durable export, and the repository contains no one-week
production dataset or recorded cold re-run comparisons. Therefore the required evidence cannot
be reconstructed or honestly synthesized from tests.
**Options:** (a) begin a monitored production observation using durable daily metric snapshots,
then perform the J3 cold re-runs; (b) use unit/integration test traffic as a proxy; (c) start
Starlark before the observation.
**Recommendation → (a).** Tests prove the collector and matching gates, not market reuse or
warm-path correctness over production traffic. Keep Session E blocked until real dated J1 data
and the J3 review are recorded here. `PROVISIONAL` — implement durable metric retention/export
before starting the observation if the runtime can restart during the week.

### F-040 — Agent-loop provider transport is ReAct JSON (amends harness A-08) — `docs/harness_new` A-08
**What:** A-08 originally required native provider tool calling and prohibited extracting tool calls from prose. That couples the agent loop to OpenAI/Anthropic/Gemini function-calling transcripts and breaks multi-turn when history pairing is imperfect; it also blocks text-only / self-hosted models.
**Options:** (a) keep native tools forever; (b) dual-mode native+ReAct behind a flag; (c) ReAct-JSON-only on the provider path while keeping internal `ToolCall`/`finish` structured.
**Recommendation → (c) accepted.** Gateway capabilities advertise `native_tools: false`, `tool_transport: "react_json"`. TS adapter encodes/decodes strict `{"thought","actions"}` JSON; Rust kernel and confirmation/gate unchanged.
**Status:** Implemented 2026-08-11.

### F-041 — Cursor-like plan/execute tools on default agent_loop — harness §7.6/§8/§9/§10
**What:** Production `agent_loop` was a flat ReAct tool loop without todo board, sub-harness spawn, reflection-on-error, or `run_program`. Spec §8 spawn/check/await and §10 Starlark were Phase 5 optional; Conductor/`TaskGraph` existed on the legacy spine only.
**Options:** (a) revive Conductor as second authority; (b) fold prosthetic kernel tools into `agent_loop` with in-memory task board + durable orchestration bag, Sol JSON `run_program` v0, Starlark gated; (c) wait for full Starlark+ProcessTree durability before any spawn.
**Recommendation → (b) accepted.** Kernel tools: `write_todos`, `update_todos`, `spawn_task`, `check_tasks`, `await_tasks`, `cancel_tasks`, `run_program`. Child capability ∩ parent; depth ≤ 3; budget carve-out; finish blocked on open todos/running children; structured reflection notices (not LLM self-grade). Child executor currently scripted stub in API (true nested LLM harness later). Starlark remains behind A-12.
**Status:** Implemented 2026-08-11 — unit vectors in `aelio-agent-loop/tests/plan_execute.rs`; soak `docs/chat-runs/2026-08-11T15-39-32-955Z/`.

### F-042 — `run_program` compute dialect `expr_v0` (PROVISIONAL) — ahead of Starlark A-12
**What:** `run_program` v0 only ran Sol JSON tool-step batches. Users expect write-code → parse → compile → run → store → reuse with correct numeric output (e.g. addition), not just tool orchestration recipes.
**Options:** (a) wait for full Starlark/HKv4; (b) add a tiny pure `expr_v0` compute dialect inside `aelio-agent-loop` with AST-as-authority hashing (F-034 spirit); (c) shell out to external eval.
**Recommendation → (b) PROVISIONAL.** `kind=compute` / `lang=expr_v0` sources parse to `ComputeAst`, hash by AST (`ast_hash` stored as `source_hash`), eval under step budget, store in `ProgramRegistry` as `ProgramBody::Compute`. Sol `steps[]` path unchanged. Starlark remains future replacement, not this dialect.
**Status:** Implemented 2026-08-12 — `aelio-agent-loop/src/compute.rs`, tests `test_run_program_compute_*`, harness `examples/run_compute_session.rs`. Superseded for language label by F-043 (expr_v0 remains accepted as input alias).

### F-043 — Starlark A-12 surface spike (Meta crate blocked) — harness §10
**What:** Long-term target is Meta `starlark` + async host bindings (A-12). Official `starlark` crates (0.8–0.13) fail to build in this workspace (hashbrown/allocative / edition conflicts under rustc 1.94, edition 2021).
**Options:** (a) block all compute until Meta crate builds; (b) ship in-tree Starlark-surface pipeline (`parse → analyze → authorize → execute`) with AST-as-authority hashing, zero host dispatch for pure programs, `lang=starlark` (`expr_v0` alias); (c) fork Meta starlark into tree.
**Recommendation → (b) PROVISIONAL.** Durable spike in `aelio-agent-loop/src/compute.rs` via `run_starlark_pipeline`. Forbidden calls (`print`/`load`/…); host paths like `api.*` analyzed then denied in pure spike. Meta crate + async host remain deferred until dependency resolves — do not claim full §10 host bindings done.
**Status:** Implemented 2026-08-12 — observation fields `pipeline`/`analyzed`/`authorized`/`host_dispatches`; harness + plan_execute vectors assert `lang=starlark` and output 42.
