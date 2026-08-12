# Turn 23: candidate-jobs
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** candidate_explore
- **expect:** candidate path or clarifies employer vs candidate
- **status:** OK
- **elapsed_ms:** 7508
## User
```text
I am a candidate — can you recommend jobs for a React developer in India?
```
## Assistant
```text
It seems you're not authorized to view job recommendations at this moment. Please check your account status or permissions.
```
## Decision trail
1. **user_intent** — candidate_explore: candidate path or clarifies employer vs candidate
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:59:14.299Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:59:21.456Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems you're not authorized to view job recommendations at this moment. Please check your account status or permissions."
  },
  {
    "at": "2026-08-12T11:59:21.456Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```