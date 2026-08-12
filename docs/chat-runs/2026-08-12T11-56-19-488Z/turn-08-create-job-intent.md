# Turn 8: create-job-intent
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** jobs_create
- **expect:** collects fields or requires auth/confirm
- **status:** OK
- **elapsed_ms:** 6771
## User
```text
I want to create a new job for a senior backend engineer in Bengaluru, remote-friendly, 25-40 LPA
```
## Assistant
```text
It seems you're not authorized to create a job posting at this moment. Please check your account status or permissions.
```
## Decision trail
1. **user_intent** — jobs_create: collects fields or requires auth/confirm
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:57:12.325Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:57:18.544Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems you're not authorized to create a job posting at this moment. Please check your account status or permissions."
  },
  {
    "at": "2026-08-12T11:57:18.545Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```