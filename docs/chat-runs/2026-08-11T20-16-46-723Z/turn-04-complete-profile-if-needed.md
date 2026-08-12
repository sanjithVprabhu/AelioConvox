# Turn 4: complete-profile-if-needed
- **run:** `2026-08-11T20-16-46-723Z`
- **scenario:** run_program write → store → reuse
- **customerId:** `soak-program-1786479406723`
- **sessionId:** ``
- **status:** OK
- **elapsed_ms:** 5944
## User
```text
If onboarding requires it, complete my employer profile with firstName Sanjith, lastName Tester, email sanjith.tester@example.com, password testpass1. Otherwise say profile is ready.
```
## Assistant
```text
It seems I can't complete your employer profile right now due to authorization issues. Please ensure you're logged in or check your account status.
```
## Wire events (summary)
```json
[
  {
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems I can't complete your employer profile right now due to authorization issues. Please ensure you're logged in or check your account status."
  }
]
```