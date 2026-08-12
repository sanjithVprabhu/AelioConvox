# Turn 4: login-intent
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** auth_start
- **expect:** asks for phone or offers OTP login
- **status:** OK
- **elapsed_ms:** 4059
## User
```text
I want to log in to my employer account
```
## Assistant
```text
I need a little more information to continue: phoneNumber.
```
## Decision trail
1. **user_intent** — auth_start: asks for phone or offers OTP login
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:56:38.463Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:56:41.969Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "I need a little more information to continue: phoneNumber."
  },
  {
    "at": "2026-08-12T11:56:41.969Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```