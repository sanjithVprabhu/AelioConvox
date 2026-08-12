# aelio-test-2 program soak — 2026-08-11T20-48-34-043Z

- target: aelio-test-2 ONLY (`http://127.0.0.1:4174`) via Convox widget
- customerId: `soak-program-t2-1786481314043`
- sessionId: ``
- evidence: program_id=prog-859118f6174bd968 hash=MISSING reuse=true demo=false

| # | case | status | ms | preview |
|---|---|---|---|---|
| 1 | write-run-store | OK | 12807 | There was an issue retrieving the demo snapshots. Please try again later. |
| 2 | reuse-program | ERROR | 14931 | There was an issue retrieving the demo snapshots again. Please try again later. |
| 3 | close | OK | 4555 | The write → store → reuse process on aelio-test-2 allows users to create a sequence of tool actions  |

## Full transcript

### Turn 1 — write-run-store (OK)

**User:** Do not log in. Do not call send_login_otp, verify_login_otp, write_todos, update_todos, or spawn_task. Call kernel tool run_program exactly once. arguments.source must be the string "{\"version\":1,\"steps\":[{\"tool\":\"demo_account_snapshot\",\"arguments\":{}},{\"tool\":\"demo_company_snapshot\",\"arguments\":{}}]}" and arguments.rationale must be "aelio-test-2 batch demo snapshots". After the Observation, call finish alone. Prefer including Observation program_id and source_hash in the finish message.

**Assistant:**

```text
There was an issue retrieving the demo snapshots. Please try again later.
```

### Turn 2 — reuse-program (ERROR)

**User:** Do not write_todos. Call kernel tool run_program once more with the identical source string and rationale "reuse aelio-test-2 demo batch". Finish quoting both source_hash values and whether they match. Include program_id verbatim.

**Assistant:**

```text
There was an issue retrieving the demo snapshots again. Please try again later.
```

### Turn 3 — close (OK)

**User:** In two sentences, summarize write → store → reuse on aelio-test-2. Repeat the program_id if you have it. Do not call tools.

**Assistant:**

```text
The write → store → reuse process on aelio-test-2 allows users to create a sequence of tool actions that can be executed and stored for future use. The program ID from the latest run is prog-859118f6174bd968.
```
