# Turn 26: demo-snapshot-ask
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** demo_tools
- **expect:** may call demo_* or explain demo snapshots
- **status:** OK
- **elapsed_ms:** 9548
## User
```text
Without logging in, can you show a demo account snapshot if that tool exists?
```
## Assistant
```text
Here is the demo account snapshot: Employer ID: 6911c03dea240111b4c94cde, Phone Number: 9876543210, Has Password: false. If you need further assistance, let me know!
```
## Decision trail
1. **user_intent** — demo_tools: may call demo_* or explain demo snapshots
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:59:40.903Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:59:50.098Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "Here is the demo account snapshot: Employer ID: 6911c03dea240111b4c94cde, Phone Number: 9876543210, Has Password: false. If you need further assistance, let me know!"
  },
  {
    "at": "2026-08-12T11:59:50.098Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```