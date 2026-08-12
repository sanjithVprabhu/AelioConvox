# Turn 1: write-run-store
- **run:** `2026-08-11T20-47-27-350Z`
- **scenario:** aelio-test-2 only — run_program write → store → reuse
- **customerId:** `soak-program-t2-1786481247350`
- **sessionId:** ``
- **status:** ERROR
- **elapsed_ms:** 5228
- **error:** missing program_id (prog-…) in assistant reply
## User
```text
Do not log in. Do not call send_login_otp, verify_login_otp, write_todos, update_todos, or spawn_task. Call kernel tool run_program exactly once. arguments.source must be the string "{\"version\":1,\"steps\":[{\"tool\":\"demo_account_snapshot\",\"arguments\":{}},{\"tool\":\"demo_company_snapshot\",\"arguments\":{}}]}" and arguments.rationale must be "aelio-test-2 batch demo snapshots". After the Observation, call finish alone. The finish message MUST include the Observation fields program_id and source_hash verbatim.
```
## Assistant
```text
Demo snapshots retrieved successfully!
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
    "contentPreview": "Demo snapshots retrieved successfully!"
  },
  {
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```