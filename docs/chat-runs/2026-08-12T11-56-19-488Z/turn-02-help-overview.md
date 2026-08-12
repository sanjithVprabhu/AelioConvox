# Turn 2: help-overview
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** capabilities
- **expect:** lists employer/candidate help areas
- **status:** OK
- **elapsed_ms:** 3933
## User
```text
what can you help me with?
```
## Assistant
```text
I can assist you with creating or managing job postings, updating your employer profile, and handling applicant statuses. Let me know what you need help with!
```
## Decision trail
1. **user_intent** — capabilities: lists employer/candidate help areas
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:56:27.519Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:56:30.899Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "I can assist you with creating or managing job postings, updating your employer profile, and handling applicant statuses. Let me know what you need help with!"
  },
  {
    "at": "2026-08-12T11:56:30.899Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```