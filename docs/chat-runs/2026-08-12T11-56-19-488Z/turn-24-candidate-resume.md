# Turn 24: candidate-resume
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** candidate_resume
- **expect:** resume help or auth for candidate tools
- **status:** OK
- **elapsed_ms:** 7176
## User
```text
help me create or list my resumes
```
## Assistant
```text
It seems you're not authorized to view or create resumes at this moment. Please check your account status or permissions.
```
## Decision trail
1. **user_intent** — candidate_resume: resume help or auth for candidate tools
2. **wire_event_counts** — typing=2, message=1
## Confirmation decisions (soak policy)
_none_
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:59:23.309Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:59:29.933Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems you're not authorized to view or create resumes at this moment. Please check your account status or permissions."
  },
  {
    "at": "2026-08-12T11:59:29.933Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```