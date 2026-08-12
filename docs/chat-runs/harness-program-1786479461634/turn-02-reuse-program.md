# Turn 2: reuse-program

- **run:** `harness-program-1786479461634`
- **scenario:** run_program write → store → reuse (harness)
- **status:** OK

## User

```text
Reuse that same stored program with identical source and confirm the source_hash matches.
```

## Assistant

```text
Reused Sol program prog-9251d473dd38d2ce (source_hash=9251d473dd38d2ce0219252faf41b9ca9e51cfe2c5897b81fdd3eea901278606). Both runs executed the same two profile steps.

Registry snapshot: {"programs":[{"program":{"rationale":"reuse stored profile batch","steps":[{"arguments":{},"tool":"get_employer_profile"},{"arguments":{},"tool":"get_company_profile"}],"version":1},"program_id":"prog-9251d473dd38d2ce","source_hash":"9251d473dd38d2ce0219252faf41b9ca9e51cfe2c5897b81fdd3eea901278606"}]}
Host tool order: ["get_employer_profile", "get_company_profile", "get_employer_profile", "get_company_profile"]
status=completed
```
