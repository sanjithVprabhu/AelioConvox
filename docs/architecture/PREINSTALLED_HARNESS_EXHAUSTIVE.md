# Exhaustive preinstalled harness list

**Authority:** `HARNESS_CONDUCTOR_VISION.md` §14.2 (minimum) + §14.2b (target catalog)  
**Status column:** what exists today as Sol seed (`sol_harness_library`) vs still required  
**Rule:** OS preinstalls cross-cutting harnesses; tenants register domain (`crud_*`, HireBoard, …)

Legend:
- **MUST** — required for Conductor arch to feel complete (preinstall)
- **SHOULD** — OS library soon after MUST
- **MAY** — later / optional / demo
- **TENANT** — not OS default; app ships it
- **Sol** — present in `aelio-kernel` `sol_harness_library` (saved Sol contract)
- **Rust only** — exists as Conductor/agent behavior, not yet a Sol body
- **Missing** — not implemented as a playable Sol harness yet

---

## 0. Preinstall tiers (how to read this)

| Tier | When it must be installed | Count (target) |
|------|---------------------------|----------------|
| **T0 — Ceiling + starters** | Day-1 OS boot | 6 MUST |
| **T1 — Core OS libraries** | Before create-harness / wide chat | ~25 SHOULD |
| **T2 — Demos / Sol coverage** | Dev + teaching; optional in prod | ~10 MAY |
| **T3 — Meta** | After Phase E | 4 MAY |
| **T4 — Tenant** | Per product | unbounded |

---

## 1. T0 — MUST preinstall (Conductor minimum)

From vision §14.2. Without these, the factory is not real.

| id | Library | Job | Status |
|----|---------|-----|--------|
| `conductor` | conductor | Top router / stack ceiling; select next harness or finish | **Rust only** (no Sol body yet) |
| `quick_reply` | reply | Short answer from context; finish | **Sol** |
| `understand_intent` | intent | Classify goal; suggest next harness | **Sol** |
| `wait_for_user` | control | Ask one question; Park; resume | **Sol** |
| `memory_attach` | memory | Search → attach → reply / return | **Sol** |
| `escalate` | conductor | No fit → cold ProposePath / tools | **Rust only** (enum choice, not Sol) |

**Optional T0 demo (vision):**

| id | Library | Job | Status |
|----|---------|-----|--------|
| `login` / `auth.otp` | tools | One real effectful OTP path | **Missing** as Sol harness (login_golden is kernel flow, not harness lib) |

---

## 2. T1 — SHOULD preinstall (full OS cross-cutting library)

From vision §14.2b. These are the exhaustive **OS** names that belong in a production preinstall set once T0 is wired to Conductor.

### 2.1 `conductor`

| id | Job | Status |
|----|-----|--------|
| `conductor` | Top router | Rust only |
| `conductor.escalate` | Safe stop / human / cold path | Missing as Sol (≈ escalate) |
| `conductor.fresh` | Clear stack → Conductor | Partial (`detect_fresh_utterance` Sol demo) |

### 2.2 `intent`

| id | Job | Status |
|----|-----|--------|
| `understand_intent` | Classify + suggest | **Sol** |
| `clarify_slot` | Ask one missing field; Park | **Sol** |
| `detect_stack_control` | stay \| exit_up \| fresh | Rust `detect_stack_control` + Sol `detect_fresh_utterance` / `fresh_then_greet` (partial) |
| `split_multi_intent` | Multi-ask → queue/seq | Missing |
| `confirm_effect` | Yes/no before write | **Sol** as `confirm_then_act` |

### 2.3 `reply`

| id | Job | Status |
|----|-----|--------|
| `quick_reply` | Short answer | **Sol** |
| `full_reply` | Longer grounded answer | **Sol** |
| `apologize_closed` | Fail-closed message | **Sol** |
| `summarize_thread` | Compress page/thread | Missing |
| `tone_rewrite` | Rewrite under voice policy | Missing |
| `greet_then_offer_help` | Greet + branch help | **Sol** (useful starter; not in §14.2b by name) |

### 2.4 `memory`

| id | Job | Status |
|----|-----|--------|
| `memory_search` | Search only | **Sol** |
| `memory_attach` | Search → shorten → attach | **Sol** (shorten stubby) |
| `memory_store_explicit` | “remember …” | Missing |
| `memory_forget_explicit` | “forget …” | Missing (agent turn has some remember/forget paths) |
| `memory_list_open_loops` | Deferred intents | Missing |
| `context_shorten` | Shrink for page budget | Missing |
| `context_pin_fact` | Pin fact on Conductor page | Missing |

### 2.5 `control`

| id | Job | Status |
|----|-----|--------|
| `wait_for_user` | Ask + Park | **Sol** |
| `return_parent` | Pop one layer | Missing as Sol (runtime stack op) |
| `return_fresh` | Clear to Conductor | Missing as Sol |
| `delegate_harness` | Push/run named child | **Sol** as `parent_waits_on_child` + `harness.invoke@1` |
| `timeout_escape` | TTL / max attempts exit | Missing |

### 2.6 `tools`

| id | Job | Status |
|----|-----|--------|
| `select_tool` | Pick tool by intent | Missing |
| `run_tool_once` | Single invoke + normalize | Partial (`once_external_stub` / `tool.act_stub@1`) |
| `tool_sequence` | Ordered multi-tool | Missing |
| `login` / `auth.otp` | OTP send/verify program | Missing as harness |
| `crud_*` | Domain CRUD | **TENANT** |

### 2.7 `calculation` / `compute`

| id | Job | Status |
|----|-----|--------|
| `calc_express` | Eval declared expression | Missing |
| `calc_aggregate` | sum/count/avg | Partial (`list_filter_keep`) |
| `calc_repeat_add` | Deterministic repeat | **Sol** |
| `calc_sum_gate` | Add + Branch demo | **Sol** (demo; keep) |
| `calc_mul_div` | mul/div + Branch | **Sol** (demo) |
| `format_number` / `format_date` / `format_phone` | Formatters | Missing |
| `parse_slots` | Extract phone/otp/ids | Missing |
| `time_of_day_context` | Business-hours context | Missing |
| `string_contains_gate` | String Branch demo | **Sol** (demo) |

### 2.8 `policy` / `safety`

| id | Job | Status |
|----|-----|--------|
| `policy_check` | Hard/soft before effect | Missing |
| `redact_pii` | Strip before prompt/log | Missing |
| `rate_limit_notice` | User-facing throttle | Missing |
| `guard_required_field` | Guard demo | **Sol** (pattern demo) |
| `budgeted_express` | Budget-wrapped say | **Sol** (pattern demo) |
| `fallback_say` | Fallback control demo | **Sol** (pattern demo) |

### 2.9 `meta` (after Phase E — not day-1 chat)

| id | Job | Status |
|----|-----|--------|
| `create_harness_draft` | Draft new harness | Missing |
| `create_harness_test` | Sandbox cases | Missing |
| `create_harness_promote` | Admin promote only | Missing |
| `list_library` | List installed for Conductor | Missing |

---

## 3. T2 — MAY preinstall (Sol coverage / teaching demos)

Useful in seed library; not all need to be Conductor-selectable in prod.

| id | Why keep | Status |
|----|----------|--------|
| `semantic_ack` | classify + Branch + Park | **Sol** |
| `parent_waits_on_child` | parent→child wait | **Sol** |
| `stack_top_a` / `stack_mid_b` / `stack_leaf_c` | nested A→B→C wait | **Sol** (see `NESTED_HARNESS_STACK.md`) |
| `once_external_stub` | Once pattern | **Sol** |

---

## 4. T4 — TENANT (never OS default)

Examples only:

| id | Product |
|----|---------|
| `login`, `list_jobs`, `create_job`, applicant pipeline, candidate tracker | HireBoard |
| Any `crud_*` | App-supplied |

---

## 5. Gap summary (what still needs to be built for “exhaustive preinstall”)

### Critical gaps (T0 incomplete as Sol)

1. **`conductor` as Sol** (or sealed pin) — today Rust `select_starter_harness`
2. **`escalate` as Sol** (or explicit Call to cold path)
3. **`login` / `auth.otp` harness** — demo effectful path

### Highest-value T1 still Missing

| Priority | ids |
|----------|-----|
| P1 | `detect_stack_control` (full stay/exit_up/fresh), `clarify_slot`, `return_parent`, `return_fresh` |
| P1 | `memory_store_explicit`, `memory_forget_explicit`, `context_shorten` |
| P1 | `select_tool`, `run_tool_once`, `login`/`auth.otp` |
| P2 | `summarize_thread`, `tone_rewrite`, `parse_slots`, formatters |
| P2 | `policy_check`, `redact_pii`, `split_multi_intent`, `timeout_escape` |
| P3 | meta create_* / list_library |

### Already in Sol seed today (30+)

Atomic + demos + **combinations** (compose via `harness.invoke@1` / Branch):

`quick_reply`, `understand_intent`, `wait_for_user`, `memory_attach`, `full_reply`, `apologize_closed`, `calc_*`, `list_filter_keep`, `string_contains_gate`, `semantic_ack`, `confirm_then_act`, `detect_fresh_utterance`, `greet_then_offer_help`, `parent_waits_on_child`, `stack_leaf_c`, `stack_mid_b`, `stack_top_a`, `fallback_say`, `guard_required_field`, `once_external_stub`, `budgeted_express`,

**Combinations added:**

| id | Combines |
|----|----------|
| `memory_then_full_reply` | memory_attach → full-style expand |
| `greet_then_quick_reply` | greet_then_offer_help → quick_reply |
| `understand_then_memory` | understand_intent → memory_attach |
| `help_router` | help? → memory_attach : quick_reply |
| `intent_then_calc` | calc keywords? → calc_sum_gate : understand_intent |
| `greet_memory_pipeline` | greet → memory_attach |
| `fresh_then_greet` | fresh? → greet : quick_reply |
| `calc_then_report_pipeline` | calc_sum_gate → calc_mul_div → report |
| `clarify_slot` | T1 ask+Park (was Missing) |
| `memory_search` | T1 search-only (was Missing) |

---

## 6. Recommended “complete OS preinstall” checklist (target names)

Copy-paste install set for a finished OS image (T0+T1 product names, aliases noted):

```text
# conductor
conductor
conductor.escalate
conductor.fresh

# intent
understand_intent
clarify_slot
detect_stack_control
split_multi_intent
confirm_effect          # alias: confirm_then_act

# reply
quick_reply
full_reply
apologize_closed
summarize_thread
tone_rewrite

# memory
memory_search
memory_attach
memory_store_explicit
memory_forget_explicit
memory_list_open_loops
context_shorten
context_pin_fact

# control
wait_for_user
return_parent
return_fresh
delegate_harness        # alias: parent_waits_on_child / harness.invoke
timeout_escape

# tools
select_tool
run_tool_once
tool_sequence
login                   # or auth.otp

# calculation
calc_express
calc_aggregate
calc_repeat_add
format_number
format_date
format_phone
parse_slots
time_of_day_context

# policy
policy_check
redact_pii
rate_limit_notice
```

**Count:** ~40 OS harness ids (before tenant `crud_*` and meta).

---

## 7. Same page

- **Exhaustive list** = §14.2b above (sections 1–4).  
- **Must have now** = T0 table.  
- **Sol today** = 20 contracts; Conductor still does not play all of them.  
- **Largest hole** = `conductor` / stack returns / memory write-forget / tools+login as Sol harnesses.

Source of Sol ids: `aelio-os/crates/aelio-kernel/src/sol_harness_lib.rs`  
Vision catalog: `docs/architecture/HARNESS_CONDUCTOR_VISION.md` §14.2 / §14.2b
