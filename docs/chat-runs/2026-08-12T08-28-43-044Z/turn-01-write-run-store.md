# Turn 1: write-run-store
- **run:** `2026-08-12T08-28-43-044Z`
- **scenario:** aelio-test-2 only — Starlark compute run_program
- **customerId:** `soak-compute-t2-1786523323044`
- **sessionId:** ``
- **status:** OK
- **elapsed_ms:** 12528
## User
```text
Do not log in. Do not call send_login_otp, verify_login_otp, write_todos, update_todos, or spawn_task. Call kernel tool run_program exactly once. arguments.source must be exactly this JSON string: "{\"kind\":\"compute\",\"lang\":\"starlark\",\"code\":\"def add(a, b):\\n  return a + b\\n\\nadd(40, 2)\\n\",\"expect\":42}" and arguments.rationale must be "starlark addition compute demo". After the Observation, call finish alone. Include Observation output, program_id, lang, and source_hash/ast_hash in the finish message.
```
## Assistant
```text
The addition compute demo completed successfully! The output is 42. Program ID: prog-e6e6791d2aef7cde, Source Hash: e6e6791d2aef7cdeed91dd380dc60d22cadfac235d923788ec67d8be78d03cab, Language: starlark.
```
## Wire events (summary)
```json
[
  {
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "type": "message",
    "role": "assistant",
    "contentPreview": "The addition compute demo completed successfully! The output is 42. Program ID: prog-e6e6791d2aef7cde, Source Hash: e6e6791d2aef7cdeed91dd380dc60d22cadfac235d923788ec67d8be78d03cab, Language: starlark."
  },
  {
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```