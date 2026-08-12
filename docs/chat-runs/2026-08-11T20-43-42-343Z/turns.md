# aelio-test-2 program soak — 2026-08-11T20-43-42-343Z

- target: aelio-test-2 ONLY (`http://127.0.0.1:4174`) via Convox widget
- customerId: `soak-program-t2-1786481022343`
- sessionId: ``
- evidence: program_id=MISSING hash=MISSING reuse=false demo=false

| # | case | status | ms | preview |
|---|---|---|---|---|
| 1 | write-run-store | ERROR | 43978 | I wasn't able to finish that request within the safe execution limits. |
| 2 | reuse-program | ERROR | 26254 | I wasn't able to finish that request within the safe execution limits. |
| 3 | close | OK | 22391 | I wasn't able to finish that request within the safe execution limits. |

## Full transcript

### Turn 1 — write-run-store (ERROR)

**User:** You finished without calling run_program. Call kernel tool run_program NOW with source {"version":1,"steps":[{"tool":"demo_account_snapshot","arguments":{}},{"tool":"demo_company_snapshot","arguments":{}}]} and rationale "forced aelio-test-2 demo". Then finish quoting Observation program_id and source_hash.

**Assistant:**

```text
I wasn't able to finish that request within the safe execution limits.
```

### Turn 2 — reuse-program (ERROR)

**User:** You finished without calling run_program. Call kernel tool run_program NOW with source {"version":1,"steps":[{"tool":"demo_account_snapshot","arguments":{}},{"tool":"demo_company_snapshot","arguments":{}}]} and rationale "forced aelio-test-2 demo". Then finish quoting Observation program_id and source_hash.

**Assistant:**

```text
I wasn't able to finish that request within the safe execution limits.
```

### Turn 3 — close (OK)

**User:** Summarize write → store → reuse in two sentences. Mention aelio-test-2 and the program_id.

**Assistant:**

```text
I wasn't able to finish that request within the safe execution limits.
```
