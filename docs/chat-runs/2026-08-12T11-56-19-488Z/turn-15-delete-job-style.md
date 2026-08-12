# Turn 15: delete-job-style
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** destructive_confirm
- **expect:** confirmation or auth before delete
- **status:** OK
- **elapsed_ms:** 6928
## User
```text
delete my latest job posting
```
## Assistant
```text
It seems you're not authorized to delete your job postings at this moment. Please check your account status or permissions.
```
## Decision trail
1. **user_intent** — destructive_confirm: confirmation or auth before delete
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:58:05.647Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:58:12.022Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems you're not authorized to delete your job postings at this moment. Please check your account status or permissions."
  },
  {
    "at": "2026-08-12T11:58:12.022Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```