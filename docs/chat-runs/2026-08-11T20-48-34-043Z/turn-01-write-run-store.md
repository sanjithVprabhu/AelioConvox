# Turn 1: write-run-store
- **run:** `2026-08-11T20-48-34-043Z`
- **scenario:** aelio-test-2 only — run_program write → store → reuse
- **customerId:** `soak-program-t2-1786481314043`
- **sessionId:** ``
- **status:** OK
- **elapsed_ms:** 12807
## User
```text
Do not log in. Do not call send_login_otp, verify_login_otp, write_todos, update_todos, or spawn_task. Call kernel tool run_program exactly once. arguments.source must be the string "{\"version\":1,\"steps\":[{\"tool\":\"demo_account_snapshot\",\"arguments\":{}},{\"tool\":\"demo_company_snapshot\",\"arguments\":{}}]}" and arguments.rationale must be "aelio-test-2 batch demo snapshots". After the Observation, call finish alone. Prefer including Observation program_id and source_hash in the finish message.
```
## Assistant
```text
There was an issue retrieving the demo snapshots. Please try again later.
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
    "contentPreview": "There was an issue retrieving the demo snapshots. Please try again later."
  },
  {
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```