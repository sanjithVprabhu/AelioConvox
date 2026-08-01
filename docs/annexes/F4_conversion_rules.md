# F4 — Conversion rule-op JSON + edge/evidence records (§13–§14)

**Sources:** §13, §14, App E, App K.

## Rule ops (closed set — sequential, non-computational)

```json
{"op":"rename",    "from":"<path>", "to":"<path>"}
{"op":"drop",      "paths":["<path>", ...]}
{"op":"keep",      "paths":["<path>", ...]}
{"op":"default",   "path":"<path>", "v": <json-literal>}   // FABRICATING
{"op":"const_set", "path":"<path>", "v": <json-literal>}   // FABRICATING
{"op":"cast",      "path":"<path>", "to":"int|float|str|bool", "mode"?: "trunc|floor|ceil|round"}
{"op":"wrap",      "path":"<path>", "into_key":"<ident>"}
{"op":"unwrap",    "path":"<path>", "from_key":"<ident>"}
{"op":"map_enum",  "path":"<path>", "table":{"<from>":"<to>", ...}}
{"op":"path_copy", "from":"<path>", "to":"<path>"}
{"op":"trim",      "path":"<path>"}
```

**Fabricating rules** (tier escalation computed, never stored as editable flag): `default`, `const_set`.
`map_enum` on unmapped value ⇒ `Convert.RuleFail` (never invents).
`cast` obeys §5.2 matrix; forbidden casts plan/apply-time reject.

## Conversion edge record

```json
{
  "edge_id": {"tenant":"t","flow_id":"login.v1","producer_nid":"n_read","consumer_nid":"n_branch"},
  "rules": [ /* Rule[] */ ],
  "read_set_signature": ["active:bool"],
  "status": "proposed|shadow|canary|promoted|suspended|rejected",
  "reach": {"reaches_write_or_external": true, "reaches_secret": false},
  "tier_required": "auto|reviewed|locked",
  "evidence_key": "ev_…",
  "kernel_version": "1",
  "sol_version": "1"
}
```

`tier_required` is **derived** at classify time from `reach` + fabricating rules — never author-writable.

## Evidence record

```json
{
  "id": "ev_…",
  "edge_id": {…},
  "distinct_input_hashes": ["blake3…", …],
  "success_count": 0,
  "failure_count": 0,
  "shadow_agreements": 0,
  "shadow_disagreements": 0,
  "canary_consumptions": 0,
  "window_start_ms": 0,
  "window_end_ms": 0
}
```

Counts inflate only on **distinct input hashes** (§16).

## App A expressible

- **e1:** `[{"op":"rename","from":"active","to":"loggedin"}]` + reach external ⇒ reviewed
- **e2:** `[{"op":"trim","path":"code"},{"op":"cast","path":"code","to":"int"}]`

## Completion check

| Check | Result |
|-------|--------|
| App A e1/e2 expressible verbatim | **PASS** (`convert_gate` tests) |
| Fabricating rules identifiable from record alone | **PASS** (`any_fabricating`) |
| Non-computation (rules don't select rules) | **PASS** by construction + property test |
