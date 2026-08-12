# Turn 18: starlark-compute
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** compute_write_run
- **expect:** run_program starlark; output 42; program_id
- **status:** OK
- **elapsed_ms:** 6852
## User
```text
Do not log in. Do not call send_login_otp or verify_login_otp. Call kernel tool run_program exactly once. arguments.source must be exactly this JSON string: "{\"kind\":\"compute\",\"lang\":\"starlark\",\"code\":\"def add(a, b):\\n  return a + b\\n\\nadd(40, 2)\\n\",\"expect\":42}" and arguments.rationale must be "soak25 starlark addition". After Observation, finish quoting output, program_id, lang, and source_hash.
```
## Assistant
```text
The result of the addition is 42. Here are the details: Output: 42, Program ID: prog-e6e6791d2aef7cde, Language: starlark, Source Hash: e6e6791d2aef7cdeed91dd380dc60d22cadfac235d923788ec67d8be78d03cab.
```
## Decision trail
1. **user_intent** — compute_write_run: run_program starlark; output 42; program_id
2. **program_id** — prog-e6e6791d2aef7cde
3. **compute_output** — mentions 42 / compute result
4. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:58:25.414Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:58:31.914Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "The result of the addition is 42. Here are the details: Output: 42, Program ID: prog-e6e6791d2aef7cde, Language: starlark, Source Hash: e6e6791d2aef7cdeed91dd380dc60d22cadfac235d923788ec67d8be78d03cab."
  },
  {
    "at": "2026-08-12T11:58:31.914Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```