# Turn 6: reuse-program
- **run:** `2026-08-11T20-14-17-310Z`
- **scenario:** run_program write → store → reuse
- **customerId:** `soak-program-1786479257310`
- **sessionId:** ``
- **status:** OK
- **elapsed_ms:** 9791
## User
```text
Reuse that same stored program: call run_program again with the identical source JSON (same two profile tools) and rationale "reuse stored profile batch". Compare the new Observation to the first run and say whether it reused the same source_hash.
```
## Assistant
```text
The attempt to reuse the stored program failed because the `get_employer_profile` step was not authorized. This means that the current lifecycle state does not grant access to this capability. Please provide your first name, last name, email, and a password (minimum 6 characters) to resolve this issue and gain access to your profiles.
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
    "contentPreview": "The attempt to reuse the stored program failed because the `get_employer_profile` step was not authorized. This means that the current lifecycle state does not grant access to this capability. Please provide your first name, last name, email, and a password (minimum 6 characters) to resolve this issue and gain access t"
  }
]
```