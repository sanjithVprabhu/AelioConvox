# F5 — `sense.v1` field types (§22.1, §8.4, §22.3)

**Sources:** §22.1, §8.4, §22.3.

## Reserved subtree

`sense` is **runtime-owned**. Planner **rejects any write** whose write-set path is under `sense` (`Policy`). Ops may **read** sense fields. Runtime rewrites sense at every turn entry including resume.

## Field table

| Path | Type | Nullable | Refreshed on resume | Notes |
|------|------|----------|---------------------|--------|
| `sense.env.now` | str (ISO-8601) | no | yes | ledgered as nondet_value if material |
| `sense.env.channel` | str | no | yes | web_widget / whatsapp / … |
| `sense.env.locale` | str | yes | yes | BCP-47 |
| `sense.env.tz` | str | yes | yes | IANA |
| `sense.session.id` | str | no | no* | *stable within session; re-asserted |
| `sense.session.turn_index` | int | no | yes | monotonic per session |
| `sense.session.last_seen_secs_ago` | int \| null | yes | yes | computed **before** freeze (§22.3) |
| `sense.user.id` | str | no | no | channel-scoped identity |
| `sense.user.tenant_id` | str | no | no | isolation key |
| `sense.flow.active_id` | str \| null | yes | yes | single active flow v0 |
| `sense.flow.pending_step` | str \| null | yes | yes | park nid / step |
| `sense.budget.calls_left` | int \| null | yes | yes | ambient turn budget (Sense) |
| `sense.budget.ms_left` | int \| null | yes | yes | active-execution meter |

## §22.3 hydration order (names only fields above)

1. Load session + user + flow_instance (+ timestamps).  
2. Compute `last_seen_secs_ago` from loaded timestamps.  
3. Write full `sense` snapshot (freeze).  
4. Only then proceed to program (no further ambient mutation of sense mid-turn except resume refresh of marked fields).

## Completion check

| Check | Result |
|-------|--------|
| Every §22.3 step names only fields defined here | **PASS** |
| Planner rejects writes under `sense` | **PASS** (`login_golden::planner_rejects_writes_under_sense`) |
