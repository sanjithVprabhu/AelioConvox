# Turn 7: list-jobs-unauth
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** jobs_list_denied_or_auth
- **expect:** deny-by-default or ask to authenticate
- **status:** OK
- **elapsed_ms:** 7042
## User
```text
can you list my job postings?
```
## Assistant
```text
It looks like you're not authorized to view job postings at this moment. Please check your account status or permissions.
```
## Decision trail
1. **user_intent** — jobs_list_denied_or_auth: deny-by-default or ask to authenticate
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:57:03.782Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:57:10.270Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "It looks like you're not authorized to view job postings at this moment. Please check your account status or permissions."
  },
  {
    "at": "2026-08-12T11:57:10.270Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```