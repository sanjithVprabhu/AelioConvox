# Turn 2: reuse-program
- **run:** `2026-08-11T20-48-34-043Z`
- **scenario:** aelio-test-2 only — run_program write → store → reuse
- **customerId:** `soak-program-t2-1786481314043`
- **sessionId:** ``
- **status:** ERROR
- **elapsed_ms:** 14931
- **error:** missing program_id (prog-…) in assistant reply
## User
```text
Do not write_todos. Call kernel tool run_program once more with the identical source string and rationale "reuse aelio-test-2 demo batch". Finish quoting both source_hash values and whether they match. Include program_id verbatim.
```
## Assistant
```text
There was an issue retrieving the demo snapshots again. Please try again later.
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
    "contentPreview": "There was an issue retrieving the demo snapshots again. Please try again later."
  },
  {
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```