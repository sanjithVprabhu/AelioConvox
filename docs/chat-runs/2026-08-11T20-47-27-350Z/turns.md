# aelio-test-2 program soak — 2026-08-11T20-47-27-350Z

- target: aelio-test-2 ONLY (`http://127.0.0.1:4174`) via Convox widget
- customerId: `soak-program-t2-1786481247350`
- sessionId: ``
- evidence: program_id=prog-859118f6174bd968 hash=present reuse=true demo=false

| # | case | status | ms | preview |
|---|---|---|---|---|
| 1 | write-run-store | ERROR | 5228 | Demo snapshots retrieved successfully! |
| 2 | reuse-program | OK | 12320 | Program ID: prog-859118f6174bd968, Source Hash: 859118f6174bd968992b175d0c0c9e2838024411f6a5f208b7c7 |
| 3 | close | OK | 5981 | The write → store → reuse process on aelio-test-2 involves creating a program with specific steps, s |

## Full transcript

### Turn 1 — write-run-store (ERROR)

**User:** Do not log in. Do not call send_login_otp, verify_login_otp, write_todos, update_todos, or spawn_task. Call kernel tool run_program exactly once. arguments.source must be the string "{\"version\":1,\"steps\":[{\"tool\":\"demo_account_snapshot\",\"arguments\":{}},{\"tool\":\"demo_company_snapshot\",\"arguments\":{}}]}" and arguments.rationale must be "aelio-test-2 batch demo snapshots". After the Observation, call finish alone. The finish message MUST include the Observation fields program_id and source_hash verbatim.

**Assistant:**

```text
Demo snapshots retrieved successfully!
```

### Turn 2 — reuse-program (OK)

**User:** Do not write_todos. Call kernel tool run_program once more with the identical source string and rationale "reuse aelio-test-2 demo batch". Finish quoting both source_hash values and whether they match. Include program_id.

**Assistant:**

```text
Program ID: prog-859118f6174bd968, Source Hash: 859118f6174bd968992b175d0c0c9e2838024411f6a5f208b7c78d50c2e7f969
```

### Turn 3 — close (OK)

**User:** In two sentences, summarize write → store → reuse on aelio-test-2. Repeat the program_id if you have it. Do not call tools.

**Assistant:**

```text
The write → store → reuse process on aelio-test-2 involves creating a program with specific steps, storing it for future use, and executing it again when needed. The program ID from the latest run is prog-859118f6174bd968.
```
