# aelio-test-2 compute soak — 2026-08-12T08-28-43-044Z

- target: aelio-test-2 ONLY (`http://127.0.0.1:4174`) via Convox widget
- customerId: `soak-compute-t2-1786523323044`
- sessionId: ``
- evidence: program_id=prog-e6e6791d2aef7cde hash=present output42=true reuse=true starlark=true

| # | case | status | ms | preview |
|---|---|---|---|---|
| 1 | write-run-store | OK | 12528 | The addition compute demo completed successfully! The output is 42. Program ID: prog-e6e6791d2aef7cd |
| 2 | reuse-program | OK | 6793 | The reused addition compute demo completed successfully! The output is 42. Program ID: prog-e6e6791d |
| 3 | close | OK | 3998 | The Starlark addition program was processed through parse, analyze, authorize, execute, store, and r |

## Full transcript

### Turn 1 — write-run-store (OK)

**User:** Do not log in. Do not call send_login_otp, verify_login_otp, write_todos, update_todos, or spawn_task. Call kernel tool run_program exactly once. arguments.source must be exactly this JSON string: "{\"kind\":\"compute\",\"lang\":\"starlark\",\"code\":\"def add(a, b):\\n  return a + b\\n\\nadd(40, 2)\\n\",\"expect\":42}" and arguments.rationale must be "starlark addition compute demo". After the Observation, call finish alone. Include Observation output, program_id, lang, and source_hash/ast_hash in the finish message.

**Assistant:**

```text
The addition compute demo completed successfully! The output is 42. Program ID: prog-e6e6791d2aef7cde, Source Hash: e6e6791d2aef7cdeed91dd380dc60d22cadfac235d923788ec67d8be78d03cab, Language: starlark.
```

### Turn 2 — reuse-program (OK)

**User:** Do not write_todos. Call kernel tool run_program once more with the identical source string and rationale "reuse starlark addition". Finish quoting output, program_id, source_hash, and whether hashes match. Confirm lang is starlark.

**Assistant:**

```text
The reused addition compute demo completed successfully! The output is 42. Program ID: prog-e6e6791d2aef7cde, Source Hash: e6e6791d2aef7cdeed91dd380dc60d22cadfac235d923788ec67d8be78d03cab. Language: starlark. Hashes match: true.
```

### Turn 3 — close (OK)

**User:** In two sentences, summarize parse→analyze→authorize→execute→store→reuse for the Starlark addition program. Quote program_id and output 42. Do not call tools.

**Assistant:**

```text
The Starlark addition program was processed through parse, analyze, authorize, execute, store, and reuse. Program ID: prog-e6e6791d2aef7cde, Output: 42.
```
