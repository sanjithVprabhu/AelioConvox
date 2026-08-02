# Aelio production / open-source readiness checklist

**Status:** active working checklist  
**Authority:** `AELIO_DSL_MOTHER.md` > this checklist > unified master plan > code comments  
**Companion audits:**
- `docs/operations/UNIFIED_COMPLETION_AUDIT_FINDINGS.md`
- `docs/new_arch/AELIO_PUBLIC_BOUNDARY_SCENARIO_EVIDENCE.md`
- `docs/claude_context/AELIO_UNIFIED_IMPLEMENTATION_MASTER_PLAN.md`
- `docs/requirements/aelio.json`

**Rule:** do not mark a gate complete from memory. Mark complete only when the acceptance column
is proven on a clean tree or green CI SHA. Prefer linking the commit / CI run.

**Release bar (consensus):**
- Aelio is a closed-authority agent OS: models propose; Rust alone guarantees and executes.
- Semantic-only flows never invent executable steps on the hot path.
- Open-source ship means measured guarantees + honest residuals, not “everything done.”
- Live provider/load/soak (`PRODUCTION-LIVE-001`) is release-CI evidence, not local green alone.

Legend: `[ ]` open · `[~]` in progress · `[x]` done · `[!]` blocked / needs decision

---

## Gate 0 — Preserve the cutover

| ID | Item | Acceptance | Status | Evidence |
|---|---|---|---|---|
| G0-1 | Safety checkpoint commit of current unified cutover (~69 modified + 11 untracked) | Commit exists; `git status` clean or only intentional follow-ups remain | [x] | `4cb8b307` |
| G0-2 | Working branch / PR opened for remediation | PR URL exists; describes Gates 1–6 | [ ] | |
| G0-3 | This checklist committed and treated as the execution index | File present under `docs/operations/` | [x] | `c894117d` + follow-up |

---

## Gate 1 — Mother P0 (blocking for Mother-complete)

| ID | Item | Acceptance | Status | Evidence |
|---|---|---|---|---|
| G1-1 | Pathway prototype cap = **8** (Mother §18) | `MAX_PROTOTYPES = 8`; validation rejects 9; tests cover 8 and 9 | [x] | views.rs + artifact_views.rs |
| G1-2 | Adaptive sealing uses Mother §4.3 **canonical Sol + BLAKE3** | `AdaptiveDecisionEnvelopeV1` no longer hashes `serde_json` with SHA-256; same logical decision ⇒ stable BLAKE3; unknown fields still rejected | [x] | adaptive.rs |
| G1-3 | Invented constants audited | Every magic number has mother cite, App/§33 constant, or FLAGS `PROVISIONAL` entry | [~] | F-021 inventory opened |
| G1-4 | No silent Mother weakenings remain in unified cutover paths | Grep/audit for known P0 sites; FLAGS for any deferral | [ ] | |

---

## Gate 2 — Authority / cutover P1 (blocking for production path)

| ID | Item | Acceptance | Status | Evidence |
|---|---|---|---|---|
| G2-1 | Production agent turn path does **not** require `dyn ToolHost` (§14.1) | Production binary constructs turns without legacy ToolHost effect dispatch; effects only via runtime-owned proxies / adaptive host; test fails if agent dispatches effects directly | [x] | `CapabilityHost` on turn/invoke/world |
| G2-2 | Unsafe SDK-only constructors fail closed or test/parity-only | `new_with_sdk_bridge` / `new_with_scoped_sdk_bridge` cannot leave `legacy_flow_execution_enabled == true` in prod builds | [x] | AppState::build always disables |
| G2-3 | Production constructor scan | Grep/source scan: every production `AppState` path disables legacy flow execution; negative test included | [x] | constructor_authority_tests |
| G2-4 | Evidence doc ↔ requirements reconciled | Delete stale “may move from partial” language; `E2E-PUBLIC-001` status matches evidence | [x] | scenario evidence |
| G2-5 | Deliberate non-goals documented | Documented: no `fallback` escape in FlowLoweringV1; adaptive hot path Pure/non-Park; live matrix external; legacy interpreter parity-only | [x] | AUTHORED_FLOW_LOWERING.md |

---

## Gate 3 — Scenario / docs integrity

| ID | Item | Acceptance | Status | Evidence |
|---|---|---|---|---|
| G3-1 | Scenario 1 cold/warm greeting | Public test asserts promotion, warm hit, zero effects, replay | [ ] | evidence matrix #1 |
| G3-2 | Scenario 2 login/OTP/park/repair/restart | Public test asserts effect counts, ledger order, reopen, replay | [ ] | #2 |
| G3-3 | Scenario 3 duplicate input + duplicate tool result | One effect; dispositions durable; replay identical | [ ] | #3 |
| G3-4 | Scenario 4 SDK disconnect pre/post dispatch | Pre: no intent; post: manual_review, no fabricated result | [ ] | #4 |
| G3-5 | Scenario 5 read-only detour + deferred second flow | Continuation byte-identical; open_loop; resume advances deferred | [ ] | #5 |
| G3-6 | Scenario 6 Prism modalities | Scalar/text/vector/graph/fusion through public `/v1/prism` | [ ] | #6 |
| G3-7 | Scenario 7 invalid Prism | Fail-closed; no row mutation | [ ] | #7 |
| G3-8 | Scenario 8 demand → build → sandbox canary | Off-path only; later pin executable | [ ] | #8 |
| G3-9 | Scenario 9 reviewed approval before consumption | Inert until deployer principal | [ ] | #9 |
| G3-10 | Scenario 10 canary promote + Guard demotion | Deduped evidence; atomic demotion | [ ] | #10 |
| G3-11 | Scenario 11 dependency/kernel migration cascades | Exact demotion + rebuild demand | [ ] | #11 |
| G3-12 | Scenario 12 cross-tenant isolation | Rows, modalities, artifacts, evidence isolated | [ ] | #12 |
| G3-13 | Scenario 13 historical replay after DB change | Injected read; no live re-read; hashes match | [ ] | #13 |
| G3-14 | Scenario 14 late result after unknown outcome | `late`; manual_review retained | [ ] | #14 |
| G3-15 | Scenario 15 graceful SIGTERM drain | Drain + restart recovers build stage | [ ] | #15 |
| G3-16 | Render Protocol F5 frame ledgering | Resume/replay reconstructs frames **or** FLAGS explicitly defers | [ ] | F-020 / express path |
| G3-17 | Dual-ladder note published | Mother §30 P0 ≠ unified master DoD explained | [ ] | ops note |

---

## Gate 4 — Reproducible local release gates

Run on a **clean** tree after remediation commits.

| ID | Item | Acceptance | Status | Evidence |
|---|---|---|---|---|
| G4-1 | `cargo test --workspace --all-features` (in `aelio-os`) | Pass | [ ] | |
| G4-2 | Multi-seed DB / Prism adversarial suites referenced by claim | Pass | [ ] | |
| G4-3 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass | [ ] | |
| G4-4 | `cargo fmt --check` + `git diff --check` | Pass | [ ] | |
| G4-5 | `pnpm typecheck` | All tasks pass | [ ] | |
| G4-6 | Requirements / convergence / production-authority validators | Pass; ledger honest | [ ] | |
| G4-7 | `pnpm test:all` | Pass (local deterministic); live tests remain ignored without creds | [ ] | |
| G4-8 | Green CI on **exact** release SHA | CI run URL linked here | [ ] | |

---

## Gate 5 — Live proof (`PRODUCTION-LIVE-001`)

Requires a real host / dummy application connected to Aelio. Do **not** mark implemented from local deterministic tests alone.

| ID | Item | Acceptance | Status | Evidence |
|---|---|---|---|---|
| G5-0 | Dummy host application available | Implements reverse-tool targets + optional LLM gateway stubs/live | [ ] | |
| G5-1 | Live login/OTP Park + wrong-code repair + restart | Matches scenario 2 under real process topology | [ ] | |
| G5-2 | Live duplicate / late tool-result dispositions | Matches scenarios 3–4 / 14 | [ ] | |
| G5-3 | Live read-only detour while parked | Matches scenario 5 | [ ] | |
| G5-4 | Live missing-capability → demand (no inline invention) | Demand record; no effect | [ ] | |
| G5-5 | Live tenant isolation attempt | Cross-tenant denied | [ ] | |
| G5-6 | Real provider path (opt-in CI) | Five live LLM tests executed with credentials | [ ] | |
| G5-7 | Channel smoke (widget and/or WhatsApp) | Message round-trip through public boundary | [ ] | |
| G5-8 | Load / soak / crash-window | Release-CI matrix; SIGTERM mid-effect; DB reopen | [ ] | |
| G5-9 | Flip `PRODUCTION-LIVE-001` only after G5-* evidence | Requirements ledger updated with CI pointers | [ ] | |

### Suggested dummy-app contract

The companion host should expose at least:

1. `otp.send` / `otp.verify` style write tools with idempotency keys  
2. One pure/read tool for parked detours  
3. Ability to drop the SDK socket before/after dispatch  
4. Ability to replay duplicate and late tool results  
5. Optional second tenant credential for isolation attacks  

Wire through `AELIO_HOST_URL` + `AELIO_HOST_TOKEN` against production-shaped `aelio-server` (no `AELIO_ALLOW_INSECURE_OPEN` for the final live gate).

---

## Gate 6 — Open-source ship bar

| ID | Item | Acceptance | Status | Evidence |
|---|---|---|---|---|
| G6-1 | README states how to run, what is guaranteed, what is not | Honest residuals listed | [ ] | |
| G6-2 | Production runbook current | Startup, drain, backup, restore, readiness | [ ] | `AELIO_PRODUCTION_RUNBOOK.md` |
| G6-3 | Authored-flow lowering guide current | Includes non-goals (`fallback`, etc.) | [ ] | `AUTHORED_FLOW_LOWERING.md` |
| G6-4 | No production-blocking unresolved FLAGS / PROVISIONAL | Triaged: closed or explicitly deferred with rationale | [ ] | `FLAGS.md` |
| G6-5 | Requirements ledger: 0 missing, 0 accidental partial | External only for truly external evidence | [ ] | `aelio.json` |
| G6-6 | Tag **release candidate** (not “everything done”) | Tag on green CI SHA | [ ] | |
| G6-7 | Open-source release notes | Guarantees + residuals + how to run live matrix | [ ] | |

---

## Broader product goals (do not regress)

These are the consensual product goals. Remediation work must preserve them.

1. **One execution authority** — Rust Planner/Executor only.  
2. **Pinned artifacts only** — canary/promoted pins; same-turn invention forbidden.  
3. **Closed adaptive decisions** — `invoke | reply | insufficient | abstain` only.  
4. **Executable authorship is explicit** — `FlowLoweringV1` HOW layer; semantic FlowSpec alone fails closed.  
5. **Runtime-owned continuations** — Park/resume/TTL/repair outside legacy `World.user_flows`.  
6. **Exactly-once effects** — intent → dispatch → result; duplicate/late classified.  
7. **Prism-only flexible recall** — multimodal, projected, bounded, tenant-scoped.  
8. **Evidence-gated learning** — propose ≠ execute; distinct evidence; reviewed consumption.  
9. **Off-path build loop** — demand → builder → sandbox → gate → canary → promote / demote.  
10. **Ops honesty** — auth fail-closed, backup/restore, drain, readiness, redacted logs.

---

## Execution order (do not reorder casually)

1. **G0** safety checkpoint + this checklist  
2. **G1** Mother P0 (cap, hash, constants)  
3. **G2** ToolHost / constructors / docs reconciliation  
4. **G3** scenario assertion audit + Render/FLAGS notes  
5. **G4** full local gates + CI on SHA  
6. **G5** dummy app + live matrix  
7. **G6** RC tag / open-source ship materials  

---

## Progress log

| Date | Gate/ID | Note |
|---|---|---|
| 2026-08-02 | — | Checklist created from consensus + Codex/Claude audit agreement. Functional local gate already green on dirty tree; Mother P0/P1 and commit/CI remain open. |
| 2026-08-02 | G0-1/G0-3 | Cutover already on `aelio-final-wrap` (`4cb8b307`); checklist committed (`c894117d`). |
| 2026-08-02 | G1-1 | Pathway `MAX_PROTOTYPES` → 8; boundary tests 8/9 added. |
| 2026-08-02 | G1-2 | `AdaptiveDecisionEnvelopeV1` seals with canonical Sol + BLAKE3. |
| 2026-08-02 | G2-1 | Production turn/effect path typed on `CapabilityHost`; raw `ToolHost::call` is parity/test extension. |
| 2026-08-02 | G2-2/G2-3 | Every `AppState::build` disables legacy flow execution; SDK-only constructors documented as parity/test; explicit `*_for_legacy_parity` opt-in for demo fixtures; constructor_authority_tests green. |
| 2026-08-02 | G2-4/G2-5 | Evidence doc stale language removed; authored-flow non-goals documented; F-020/F-021 residuals flagged. |

---

## Definition of “wrap-up complete”

All of the following are true:

- Gates **0–4** and **6** are `[x]`  
- Gate **5** is either `[x]` or still honestly `external` with a dated plan and CI hooks  
- No checklist row that blocks Mother-complete or production path remains `[ ]`  
- Release language never claims live evidence the repo does not have  
