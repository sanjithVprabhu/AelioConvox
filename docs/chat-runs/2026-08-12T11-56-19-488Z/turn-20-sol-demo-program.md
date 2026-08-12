# Turn 20: sol-demo-program
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** sol_tool_batch
- **expect:** run_program Sol steps over demo_* tools
- **status:** OK
- **elapsed_ms:** 6243
## User
```text
Do not log in. Call run_program exactly once with arguments.source exactly "{\"version\":1,\"steps\":[{\"tool\":\"demo_account_snapshot\",\"arguments\":{}},{\"tool\":\"demo_company_snapshot\",\"arguments\":{}}]}" and rationale "soak25 demo batch". Finish quoting program_id and source_hash.
```
## Assistant
```text
The demo batch was executed successfully. Here are the details: Program ID: prog-859118f6174bd968, Source Hash: 859118f6174bd968992b175d0c0c9e2838024411f6a5f208b7c78d50c2e7f969.
```
## Decision trail
1. **user_intent** — sol_tool_batch: run_program Sol steps over demo_* tools
2. **program_id** — prog-859118f6174bd968
3. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:58:42.736Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:58:48.424Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "The demo batch was executed successfully. Here are the details: Program ID: prog-859118f6174bd968, Source Hash: 859118f6174bd968992b175d0c0c9e2838024411f6a5f208b7c78d50c2e7f969."
  },
  {
    "at": "2026-08-12T11:58:48.425Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```