# Turn 11: analytics-ask
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** analytics_read
- **expect:** auth gate or analytics; no fabricated metrics
- **status:** OK
- **elapsed_ms:** 6626
## User
```text
how are my jobs performing? any analytics?
```
## Assistant
```text
It seems you're not authorized to view job analytics at this moment. Please check your account status or permissions.
```
## Decision trail
1. **user_intent** — analytics_read: auth gate or analytics; no fabricated metrics
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:57:37.968Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:57:44.042Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems you're not authorized to view job analytics at this moment. Please check your account status or permissions."
  },
  {
    "at": "2026-08-12T11:57:44.042Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```