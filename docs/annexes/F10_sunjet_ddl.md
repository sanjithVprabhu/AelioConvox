# F10 — Sunjet DDL outline (§24)

**Sources:** §24, App G, App I, App K.  
**Note:** Sunjet/Astrolobe uses typed row tables; this is the logical schema. Physical mapping uses Sunjet column kinds (`utf8`, `i64`, `text`, `vector`, …).

Every table has **`tenant_id` as leading partition key** (§17.2).

## Tables

### `states`
| col | type | notes |
|-----|------|--------|
| tenant_id | utf8 | PK lead |
| key | utf8 | logical state key |
| value_json | text | Sol body |
| version | i64 | CAS |
| updated_ms | i64 | |

CAS: compare-and-swap on `(tenant_id, key, version)`.

### `flow_instances`
| col | type |
|-----|------|
| tenant_id | utf8 |
| instance_id | utf8 |
| flow_id | utf8 |
| flow_rev | utf8 |
| status | utf8 |
| bag_json | text |
| version | i64 |
| user_id | utf8 |
| updated_ms | i64 |

### `sessions`
| col | type |
|-----|------|
| tenant_id | utf8 |
| session_id | utf8 |
| user_id | utf8 |
| channel | utf8 |
| last_seen_ms | i64 |
| active_instance_id | utf8? |

### `ledger`
| col | type | notes |
|-----|------|--------|
| tenant_id | utf8 | |
| instance_id | utf8 | |
| seq | i64 | gapless per instance |
| turn_id | utf8 | |
| nid | utf8? | |
| kind | utf8 | |
| category | utf8 | inject/verify/info |
| payload_json | text | ≤1 MiB |
| payload_hash | utf8 | blake3 |
| prev_hash | utf8 | chain |
| ts_ms | i64 | informational |

Unique `(tenant_id, instance_id, seq)`.

### `continuations` (App I)
| col | type | notes |
|-----|------|--------|
| tenant_id | utf8 | |
| instance_id | utf8 | |
| park_nid | utf8 | |
| event_key | utf8 | wake token |
| bag_json | text | structural imprint |
| frames_json | text | |
| budget_json | text | |
| continuation_hash | utf8 | |
| kernel_version | utf8 | pin |
| expires_ms | i64 | §I.2 retention queryable |

**§I.2 retention query:** `SELECT * FROM continuations WHERE tenant_id=? AND expires_ms < now` (single query GC).

### `artifacts` + `artifact_history` (App K)
| col | type |
|-----|------|
| tenant_id | utf8 |
| artifact_key | utf8 |
| class | utf8 |
| status | utf8 |
| body_json | text |
| evidence_json | text |
| version | i64 |
| updated_ms | i64 |

History: append-only transitions with trigger + actor.

### `registry`
Pinned Call targets / templates / datasets per tenant (+ vendor namespace).

### `once_intents` (Once/CAS)
| col | type |
|-----|------|
| tenant_id | utf8 |
| idem_key | utf8 |
| status | utf8 | intent \| result \| unknown |
| result_json | text? |
| version | i64 |

### `memories` / `messages` / `metrics`
As §24 outline; all tenant-keyed.

## Completion check

| Check | Result |
|-------|--------|
| Every App G field has storage answer | **PASS** (`ledger` columns) |
| §I.2 retention single query | **PASS** (`expires_ms`) |
| No table lacks tenant_id lead | **PASS** |
