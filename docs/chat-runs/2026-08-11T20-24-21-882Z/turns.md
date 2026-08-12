# aelio-test-2 program soak — 2026-08-11T20-24-21-882Z

- target: aelio-test-2 ONLY (`http://127.0.0.1:4174`) via Convox widget
- customerId: `soak-program-t2-1786479861882`
- sessionId: ``
- evidence: program_id=MISSING hash=MISSING reuse=false demo=false

| # | case | status | ms | preview |
|---|---|---|---|---|
| 1 | intent | OK | 7494 | Demo program executed successfully, showcasing account and company snapshots. |
| 2 | write-run-store | OK | 5181 | Demo program executed successfully, showcasing account and company snapshots. |
| 3 | reuse-program | ERROR | 35547 | Something went wrong processing your message. Please try again. |
| 4 | close | ERROR | 536 | Something went wrong processing your message. Please try again. |

## Full transcript

### Turn 1 — intent (OK)

**User:** Do not log me in. Do not call send_login_otp or verify_login_otp. I only want a run_program write/store/reuse demo on aelio-test-2 using demo_account_snapshot and demo_company_snapshot.

**Assistant:**

```text
Demo program executed successfully, showcasing account and company snapshots.
```

### Turn 2 — write-run-store (OK)

**User:** Call run_program exactly once now. arguments.source must be this JSON string: {"version":1,"steps":[{"tool":"demo_account_snapshot","arguments":{}},{"tool":"demo_company_snapshot","arguments":{}}]} arguments.rationale must be "aelio-test-2 batch demo snapshots". After the Observation, call finish alone and quote program_id and source_hash from that Observation. Do not call demo_* tools directly — only via run_program steps.

**Assistant:**

```text
Demo program executed successfully, showcasing account and company snapshots.
```

### Turn 3 — reuse-program (ERROR)

**User:** Call run_program again with the identical source JSON and rationale "reuse aelio-test-2 demo batch". Finish quoting both source_hash values and say if they match. No OTP tools.

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 4 — close (ERROR)

**User:** Summarize write → store → reuse for that Sol program in two sentences. Mention aelio-test-2.

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```
