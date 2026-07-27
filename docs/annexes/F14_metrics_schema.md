# F14 — Metrics schema (§21, §18, App K)

**Sources:** §21, §18, App K.

| Metric | Type | Labels | Window | Source event |
|--------|------|--------|--------|--------------|
| `turns_total` | counter | tenant, channel | ∞ / 24h | ledger `turn_start` |
| `llm_calls_total` | counter | tenant, model | 24h | ledger `model_call` |
| `tool_calls_total` | counter | tenant, target, outcome | 24h | ledger `call_result` |
| `tier_hits` | counter | tenant, tier (0\|1\|2\|3) | 24h | pathway/procedure pick ledger |
| `converter_warm_hit_rate` | gauge | tenant | 90d | artifact_history promote + convert firings |
| `converter_shadow_agree_rate` | gauge | tenant, edge | 30d | evidence.shadow_* |
| `pathway_fallback_rate` | gauge | tenant, decision_point | 7d | ledger `pathway_pick.fallback_used` |
| `pathway_margin` | histogram | tenant, decision_point | 24h | ledger `pathway_pick.margin` |
| `park_abandoned` | counter | tenant, flow | 30d | termination turn |
| `once_unknown_outcome` | counter | tenant | 24h | Once Internal raises |
| `replay_divergence` | counter | tenant | 24h | hard refuse events |
| `budget_trips` | counter | tenant, meter | 24h | ledger Budget/Timeout trips |
| `promotion_events` | counter | tenant, class, result | 90d | artifact_history |
| `procedure_turn_share` | gauge | tenant | 6mo | turns using promoted procedures / turns |

## Kill criteria computable from metrics alone (§21)

1. **Converters warm-hit @ 90d < 60%** ⇒ core bet falsified — uses `converter_warm_hit_rate`.  
2. **Procedures share of turns @ 6mo < 10%** ⇒ demote from headline — uses `procedure_turn_share`.

## §18 fallback rate

`pathway_fallback_rate` = count(pathway_pick where fallback_used) / count(pathway_pick) over 7d — both from ledger `pathway_pick`.

## Completion check

| Check | Result |
|-------|--------|
| Both §21 kill criteria computable | **PASS** |
| §18 fallback rate defined | **PASS** |
| No metric lacks source event | **PASS** |
