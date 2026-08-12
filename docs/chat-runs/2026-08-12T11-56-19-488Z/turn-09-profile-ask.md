# Turn 9: profile-ask
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** profile_read
- **expect:** auth gate or real profile; no inventing private data
- **status:** OK
- **elapsed_ms:** 7572
## User
```text
show me my employer profile
```
## Assistant
```text
It seems you're not authorized to view your employer profile at this moment. Please check your account status or permissions.
```
## Decision trail
1. **user_intent** — profile_read: auth gate or real profile; no inventing private data
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:57:20.599Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:57:27.617Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems you're not authorized to view your employer profile at this moment. Please check your account status or permissions."
  },
  {
    "at": "2026-08-12T11:57:27.617Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```