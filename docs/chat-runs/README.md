# Chat soak runs

Latest successful thorough run:

- [`2026-08-11T10-51-08-485Z/SUMMARY.md`](./2026-08-11T10-51-08-485Z/SUMMARY.md)
- Full transcript: [`2026-08-11T10-51-08-485Z/turns.md`](./2026-08-11T10-51-08-485Z/turns.md)
- Per-turn files: `2026-08-11T10-51-08-485Z/turn-NN-*.md`

## How to reproduce

```bash
# Terminal 1
AELIO_SKIP_EXAMPLE_SDK=1 AELIO_HARNESS_MODE=agent_loop pnpm start

# Terminal 2
cd ../AelioDev/aelio-test-2
AELIO_SERVER_URL=ws://127.0.0.1:3010 AELIO_SECRET=change-me-in-production npm start

# Terminal 3 — 15-turn smoke
node scripts/chat-soak-15.mjs

# Or 28-scenario thorough soak (master MD with chats + decisions + wire logs)
node scripts/chat-soak-25.mjs
```

## Fixes exercised by these runs

1. **Multi-turn OpenAI tool_call pairing** — accepted `finish` (and missing-arg parks) now append matching `tool_result`s in `aelio-agent-loop` so turn 2+ no longer fail with unpaired `tool_call_id`.
2. **Confirmation handling in soak** — write tools park for yes/no; the soak auto-sends `yes` and continues.
3. **Widget confirmation classification** — missing-input parks are no longer mislabeled as action confirmations solely because `suspended=true`.
- [`2026-08-11T11-42-24-758Z/`](./2026-08-11T11-42-24-758Z/) — ReAct agent_loop soak, **PASS** 15/15 (~68s)
- [`2026-08-11T15-39-32-955Z/`](./2026-08-11T15-39-32-955Z/) — Cursor-like plan/execute kernel tools on ReAct agent_loop, **PASS** 15/15
- [`2026-08-11T19-21-22-875Z/`](./2026-08-11T19-21-22-875Z/) — aelio-test-2 live soak on plan/execute ReAct loop, **PASS** 15/15 (post-OTP stuck asking for mobile)
- [`2026-08-11T20-14-17-310Z/`](./2026-08-11T20-14-17-310Z/) — live LLM program soak (auth blocked profile tools; no program_id)
- [`2026-08-11T20-16-46-723Z/`](./2026-08-11T20-16-46-723Z/) — live LLM program soak v2 split OTP (OTP verify failed; no program_id)
- [`harness-program-1786479461634/`](./harness-program-1786479461634/) — harness `run_program` write→store→reuse **PASS** (`prog-9251d473dd38d2ce`, 4 host tool calls)
- [`2026-08-11T21-19-34-121Z/`](./2026-08-11T21-19-34-121Z/) — **aelio-test-2 only** live `run_program` write→store→reuse **PASS** (`prog-859118f6174bd968`); script: `node scripts/chat-soak-run-program.mjs`
- [`harness-compute-1786521067101/`](./harness-compute-1786521067101/) — early **expr_v0** compute addition harness **PASS**
- [`harness-compute-1786521972090/`](./harness-compute-1786521972090/) — **Starlark-surface A-12** addition harness **PASS** (`cargo run -p aelio-agent-loop --example run_compute_session`); parse→analyze→authorize→execute → output 42 → store → reuse; `lang=starlark`
- Live compute soak: `node scripts/chat-soak-compute.mjs` (aelio-test-2 only; asserts output 42 + `program_id`)
- [`2026-08-12T08-28-43-044Z/`](./2026-08-12T08-28-43-044Z/) — **aelio-test-2 only** live Starlark compute **PASS** (`prog-e6e6791d2aef7cde`, output 42, reuse same hash)
- [`2026-08-12T11-56-19-488Z/`](./2026-08-12T11-56-19-488Z/) — **28-scenario** thorough soak **PASS** (~3.7 min); master report [`ALL_CHATS_DECISIONS_AND_LOGS.md`](./2026-08-12T11-56-19-488Z/ALL_CHATS_DECISIONS_AND_LOGS.md); script: `node scripts/chat-soak-25.mjs`

## Dialects

| Dialect | Purpose | Long-term |
|--------|---------|-----------|
| Sol `steps[]` | Tool-batch recipes under effect gate | Keep |
| `lang=starlark` (A-12 surface spike) | Pure compute: write code → AST hash → eval → store | Meta `starlark` crate + async host when deps build (F-043) |
| `lang=expr_v0` | Accepted input alias for the same surface | Deprecated label |