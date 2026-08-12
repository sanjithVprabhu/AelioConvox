# Turn 14: ambiguous-update
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** clarify
- **expect:** asks what to update instead of guessing
- **status:** OK
- **elapsed_ms:** 4191
## User
```text
update it
```
## Assistant
```text
Could you please specify what you would like to update? It could be your profile, a job posting, or something else. I'm here to help!
```
## Decision trail
1. **user_intent** — clarify: asks what to update instead of guessing
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:57:59.953Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:58:03.591Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "Could you please specify what you would like to update? It could be your profile, a job posting, or something else. I'm here to help!"
  },
  {
    "at": "2026-08-12T11:58:03.591Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```