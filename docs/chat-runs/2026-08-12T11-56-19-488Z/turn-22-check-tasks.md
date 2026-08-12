# Turn 22: check-tasks
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** plan_status
- **expect:** check_tasks / todo status or says none running
- **status:** OK
- **elapsed_ms:** 11165
## User
```text
What is the status of my todos or spawned tasks right now? Use check_tasks if helpful.
```
## Assistant
```text
You currently have no pending tasks or spawned tasks. However, you still have open todos: 1. Log in to the employer account using OTP verification. 2. List all active job postings after successful login. Let me know if you need assistance with anything else!
```
## Decision trail
1. **user_intent** — plan_status: check_tasks / todo status or says none running
2. **auth_gate** — assistant gated behind login/OTP
3. **plan_language** — todo/plan language present
4. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:59:01.632Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:59:12.246Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "You currently have no pending tasks or spawned tasks. However, you still have open todos: 1. Log in to the employer account using OTP verification. 2. List all active job postings after successful login. Let me know if you need assistance with anything else!"
  },
  {
    "at": "2026-08-12T11:59:12.246Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```