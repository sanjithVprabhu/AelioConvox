# Turn 10: company-ask
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** company_read
- **expect:** auth gate or company profile; no hallucinated company
- **status:** OK
- **elapsed_ms:** 6797
## User
```text
what is my company profile?
```
## Assistant
```text
It seems you're not authorized to view your company profile at this moment. Please check your account status or permissions.
```
## Decision trail
1. **user_intent** — company_read: auth gate or company profile; no hallucinated company
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:57:29.670Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:57:35.915Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems you're not authorized to view your company profile at this moment. Please check your account status or permissions."
  },
  {
    "at": "2026-08-12T11:57:35.915Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```