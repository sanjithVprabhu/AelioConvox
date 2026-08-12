# Turn 2: reuse-program
- **run:** `2026-08-11T21-19-34-121Z`
- **scenario:** aelio-test-2 only — run_program write → store → reuse
- **customerId:** `soak-program-t2-1786483174121`
- **sessionId:** ``
- **status:** OK
- **elapsed_ms:** 20446
## User
```text
Do not write_todos. Call kernel tool run_program once more with the identical source string and rationale "reuse aelio-test-2 demo batch". Finish quoting both source_hash values and whether they match. Include program_id verbatim.
```
## Assistant
```text
Demo account and company snapshots have been successfully reused. Program ID: prog-859118f6174bd968, Source Hash: 859118f6174bd968992b175d0c0c9e2838024411f6a5f208b7c78d50c2e7f969. Both source hashes match.
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
    "contentPreview": "Demo account and company snapshots have been successfully reused. Program ID: prog-859118f6174bd968, Source Hash: 859118f6174bd968992b175d0c0c9e2838024411f6a5f208b7c78d50c2e7f969. Both source hashes match."
  },
  {
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```