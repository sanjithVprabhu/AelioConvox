# F6 — Registry entry JSON per class (§10)

**Sources:** §10.1, §10.3, §10.4, App J.

## Common envelope

```json
{
  "id": "<name>",
  "version": "1",
  "class": "compute|io|tool|model|flow|dataset|template_layer",
  "input_imprint": "declared.id@v | ~structural",
  "output_imprint": "declared.id@v",
  "boundedness": {
    "kind": "cost_envelope|deadline_compliant|registered_flow",
    "max_ms"?: 20000,
    "max_calls"?: 3
  },
  "effect_class": "pure|read|write|external",
  "policy_tags": ["auth.otp.send"],
  "tenant_scope": "tenant|vendor",
  "origin": "tenant|vendor"
}
```

## Per class

### compute (validator)
```json
{"id":"validate_phone","version":"1","class":"compute","effect_class":"pure",
 "input_imprint":"phone_in@1","output_imprint":"phone_e164@1",
 "boundedness":{"kind":"cost_envelope"},"policy_tags":[],"tenant_scope":"vendor","origin":"vendor"}
```

### io prepared
```json
{"id":"state_read","version":"1","class":"io","effect_class":"read",
 "input_imprint":"state_key@1","output_imprint":"auth_raw@1",
 "boundedness":{"kind":"deadline_compliant","max_ms":5000},
 "policy_tags":["state.read"],"tenant_scope":"tenant","origin":"tenant"}
```

### io `sunjet.query`
```json
{"id":"sunjet.query","version":"1","class":"io","effect_class":"read",
 "input_imprint":"query_ast@1","output_imprint":"query_result@1",
 "boundedness":{"kind":"deadline_compliant","max_ms":10000},
 "policy_tags":["sunjet.read"],"tenant_scope":"tenant","origin":"vendor"}
```

### tool
```json
{"id":"send_otp","version":"1","class":"tool","effect_class":"external",
 "input_imprint":"e164@1","output_imprint":"otp_sent@1",
 "boundedness":{"kind":"deadline_compliant","max_ms":20000},
 "policy_tags":["auth.otp.send"],"tenant_scope":"tenant","origin":"tenant"}
```

### model (+ template pin)
```json
{"id":"ask_phone","version":"1","class":"model","effect_class":"read",
 "input_imprint":"ask_phone_args@1","output_imprint":"utterance@1",
 "template_id":"ask_phone.tmpl@1","model_id":"tier:cheap@1",
 "slots":[{"name":"lang","type":"str","sensitivity":"none"}],
 "boundedness":{"kind":"deadline_compliant","max_ms":30000},
 "policy_tags":["model.express"],"tenant_scope":"tenant","origin":"tenant"}
```

### registered-flow
```json
{"id":"authed_menu","version":"1","class":"flow","effect_class":"read",
 "input_imprint":"empty@1","output_imprint":"menu@1",
 "boundedness":{"kind":"registered_flow"},
 "policy_tags":[],"tenant_scope":"tenant","origin":"tenant"}
```

### dataset
```json
{"id":"clients","version":"1","class":"dataset",
 "modalities":["get","range","topk_vector"],
 "tenant_scope":"tenant","origin":"tenant"}
```

### template_layer (App J)
```json
{"id":"policy_preamble","version":"1","class":"template_layer",
 "text":"…","slots":[],"tenant_scope":"tenant","origin":"tenant"}
```

## App A Call targets

| Target | Class | effect_class |
|--------|-------|--------------|
| `io.sense_hydrate@1` | io | read |
| `io.state_read@1` | io | read |
| `io.state_write@1` | io | write |
| `model.ask_phone@1` | model | read |
| `model.ask_otp@1` | model | read |
| `model.express_success@1` | model | read |
| `compute.validate_phone@1` | compute | pure |
| `tool.send_otp@1` | tool | external |
| `tool.verify_otp@1` | tool | external |
| `flow.authed_menu@1` | flow | read |

## Completion check

| Check | Result |
|-------|--------|
| Every App A Call target expressible | **PASS** |
| Boundedness + effect_class on all | **PASS** |
