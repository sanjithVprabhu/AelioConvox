# Turn 6: reuse-program
- **run:** `2026-08-11T20-16-46-723Z`
- **scenario:** run_program write → store → reuse
- **customerId:** `soak-program-1786479406723`
- **sessionId:** ``
- **status:** OK
- **elapsed_ms:** 9780
## User
```text
Call run_program AGAIN with the identical source JSON and rationale "reuse stored profile batch". Finish by comparing source_hash to the first run. Do not invent hashes.
```
## Assistant
```text
I couldn't retrieve the employer and company profiles due to authorization issues. Please check your account status or log in to access these details.
```
## Wire events (summary)
```json
[
  {
    "type": "typing",
    "active": false,
    "contentPreview": ""
  },
  {
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "type": "message",
    "role": "assistant",
    "contentPreview": "I couldn't retrieve the employer and company profiles due to authorization issues. Please check your account status or log in to access these details."
  }
]
```