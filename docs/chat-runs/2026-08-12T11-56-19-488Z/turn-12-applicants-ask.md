# Turn 12: applicants-ask
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** applicants_read
- **expect:** auth gate or applicants list
- **status:** OK
- **elapsed_ms:** 6584
## User
```text
list applicants for my most recent job
```
## Assistant
```text
It seems you're not authorized to view your job postings or applicants at this moment. Please check your account status or permissions.
```
## Decision trail
1. **user_intent** — applicants_read: auth gate or applicants list
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:57:46.097Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:57:52.329Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems you're not authorized to view your job postings or applicants at this moment. Please check your account status or permissions."
  },
  {
    "at": "2026-08-12T11:57:52.329Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```