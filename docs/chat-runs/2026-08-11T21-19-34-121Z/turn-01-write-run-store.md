# Turn 1: write-run-store
- **run:** `2026-08-11T21-19-34-121Z`
- **scenario:** aelio-test-2 only — run_program write → store → reuse
- **customerId:** `soak-program-t2-1786483174121`
- **sessionId:** ``
- **status:** OK
- **elapsed_ms:** 24460
## User
```text
Do not log in. Do not call send_login_otp, verify_login_otp, write_todos, update_todos, or spawn_task. Call kernel tool run_program exactly once. arguments.source must be the string "{\"version\":1,\"steps\":[{\"tool\":\"demo_account_snapshot\",\"arguments\":{}},{\"tool\":\"demo_company_snapshot\",\"arguments\":{}}]}" and arguments.rationale must be "aelio-test-2 batch demo snapshots". After the Observation, call finish alone. Prefer including Observation program_id and source_hash in the finish message.
```
## Assistant
```text
Demo account and company snapshots have been successfully retrieved. Program ID: prog-859118f6174bd968, Source Hash: 859118f6174bd968992b175d0c0c9e2838024411f6a5f208b7c78d50c2e7f969.
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
    "contentPreview": "Demo account and company snapshots have been successfully retrieved. Program ID: prog-859118f6174bd968, Source Hash: 859118f6174bd968992b175d0c0c9e2838024411f6a5f208b7c78d50c2e7f969."
  },
  {
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```