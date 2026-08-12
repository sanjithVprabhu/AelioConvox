# aelio-test-2 program soak — 2026-08-11T20-22-34-307Z

- target: aelio-test-2 ONLY (`http://127.0.0.1:4174`) via Convox widget
- customerId: `soak-program-t2-1786479754307`
- sessionId: ``
- scenario: run_program write → store → reuse (demo_* tools)
- evidence: program_id=MISSING hash=MISSING reuse=false demo_payload=false

| # | case | status | ms | assistant preview |
|---|---|---|---|---|
| 1 | greeting | OK | 7036 | The demo account and company snapshots were successfully retrieved. You can now proceed with your te |
| 2 | write-run-store | ERROR | 40840 | Something went wrong processing your message. Please try again. |
| 3 | confirm-stored | ERROR | 61 | Something went wrong processing your message. Please try again. |
| 4 | reuse-program | ERROR | 39 | Something went wrong processing your message. Please try again. |
| 5 | close | ERROR | 39 | Something went wrong processing your message. Please try again. |

## Full transcript

### Turn 1 — greeting (OK)

**User:** hi — I am testing reusable Sol programs on aelio-test-2 using the local demo snapshot tools only.

**Assistant:**

```text
The demo account and company snapshots were successfully retrieved. You can now proceed with your testing!
```

### Turn 2 — write-run-store (ERROR)

**User:** IMPORTANT: Call the kernel tool run_program now (do not only describe it). Use source {"version":1,"steps":[{"tool":"demo_account_snapshot","arguments":{}},{"tool":"demo_company_snapshot","arguments":{}}]} and rationale "aelio-test-2 batch demo snapshots". Then finish with the Observation program_id and source_hash exactly.

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 3 — confirm-stored (ERROR)

**User:** Confirm storage. Quote program_id and source_hash from Observations only. Summarize both demo tool results briefly.

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 4 — reuse-program (ERROR)

**User:** Call run_program AGAIN with the identical source JSON and rationale "reuse aelio-test-2 demo batch". Finish by stating whether source_hash matched the first run. Do not invent hashes.

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 5 — close (ERROR)

**User:** In two sentences, summarize write → store → reuse for that Sol program on aelio-test-2.

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```
