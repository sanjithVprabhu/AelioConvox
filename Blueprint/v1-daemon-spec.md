# Aelio Daemon — Reflection & Self-Improvement Loop (V1 lite)

**Status:** Build spec for the first lite version
**Default:** OFF (opt-in via config) — zero behavior change unless enabled

---

## 1. What it is

A supervised background loop on the Aelio server — a **harness**, not a raw
`while(true)` — that lets the system improve itself between conversations. After a
customer session ends, the daemon "reflects": it asks the LLM whether the session
actually resolved the user's intent, what was learned, and what went wrong, then
writes that understanding back into the customer's long-term memory so future
turns are better.

This is the offline, self-managing complement to the live conversational agent.

```
session ends ──► daemon wakes ──► reflect (LLM-as-judge) ──► store verdict
     ▲                                                              │
     └──────────── better future turns ◄── insight → memory ◄──────┘
```

## 2. The harness (how the loop is safe)

A good harness is scheduled, budgeted, idempotent, and interruptible — never a tight
spin that burns tokens:

- **Scheduled**, not spinning: runs every `interval_minutes` (default 15), `unref`'d.
- **Budgeted**: at most `max_per_cycle` reflections per wake-up (default 5). Each
  reflection is a single, small-token LLM call.
- **Idempotent (its own cache)**: a session is reflected exactly once. The daemon
  selects only `closed` sessions with no existing `reflections` row, so re-running
  never duplicates work or LLM calls. This *is* the "don't keep calling the LLM"
  cache for the loop.
- **Crash-safe**: a failure on one session is logged and skipped, never fatal.
- **Off by default**: `daemon.enabled = false` until the operator opts in.

## 3. Reflection (the "dream state")

For each unreflected closed session the daemon:

1. Loads the transcript (messages) + the `function_calls` audit for that session.
2. Asks the LLM, in one bounded call, for a structured verdict:
   - `outcome`: `resolved | unresolved | unclear`
   - `score`: 0..1 quality/confidence
   - `summary`: 1–2 sentences of what happened
   - `issues`: list of problems (wrong tool, missing info, user frustration…)
   - `insight`: one durable fact worth remembering about this customer (optional)
3. Stores the verdict in a new `reflections` table (audit of self-evaluation).
4. If an `insight` was produced, writes it to long-term `memory` (vector-indexed),
   so the next conversation recalls it — closing the self-improvement loop.

Robust parsing: the model is asked for JSON; if it returns prose, we fall back to
storing the prose as the summary with `outcome = unclear`.

## 4. Data model

```sql
CREATE TABLE reflections (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  customer_id TEXT NOT NULL,
  outcome TEXT NOT NULL,          -- resolved | unresolved | unclear
  score REAL,
  summary TEXT,
  issues TEXT,                    -- JSON array
  created_at INTEGER NOT NULL
);
```

Created on startup alongside the vector store (same pattern as `memory_vec`), so no
migration churn. Selecting work = closed sessions whose `id` has no `reflections` row.

## 5. Config

```yaml
daemon:
  enabled: false           # opt-in
  interval_minutes: 15
  max_per_cycle: 5
  reflect_min_messages: 2  # skip trivial sessions
```

## 6. Observability

Every reflection is a structured log line and a `reflections` row — latency, outcome,
score, issues. This is exactly the execution-telemetry surface discussed earlier and
can later be exported (OTel) to an observability backend.

## 7. Proactive outbound (shipped)

A system-initiated message path, gated by guardrails enforced server-side before
anything is delivered (reusing the same outbound path as replies):

1. feature enabled (`proactive.enabled`)
2. known recipient (resolved customer + channel address)
3. opt-in (`proactive.require_opt_in`; per-customer flag)
4. dedup (`dedupKey` — never send the same thing twice)
5. daily frequency cap (`max_per_customer_per_day`)
6. WhatsApp 24-hour window — free-form only inside it; a `templateName` is required
   outside it (matches Meta's rules)

Trigger surface: authenticated HTTP `POST /proactive` and `POST /proactive/opt-in`
(the dev's backend calls these on its own domain events, e.g. "order shipped").
Every attempt — sent or blocked — is recorded in the `proactive_messages` table.

Config:

```yaml
proactive:
  enabled: false           # opt-in
  require_opt_in: true
  max_per_customer_per_day: 5
  window_hours: 24
```

## 8. Autonomous follow-up (shipped)

The loop closes itself: when a reflection comes back `unresolved`, the reflection
also drafts a short re-engagement message (`followup`, produced in the same LLM call).
If `daemon.proactive_followup` is on, the daemon routes that draft through
`sendProactiveMessage` — so it passes **every** proactive guardrail (opt-in, dedup,
frequency cap, 24h window). A per-session `dedupKey` (`followup:<sessionId>`) means a
session is followed up at most once, ever.

```yaml
daemon:
  enabled: true
  proactive_followup: true   # requires proactive.enabled
```

Flow: session ends → daemon reflects → `unresolved` + follow-up draft → guard-railed
proactive send. An opted-out customer's follow-up is blocked, exactly like a manual
proactive send.

## 9. Semantic response cache (shipped)

A per-customer cache that serves a near-identical, recent reply without calling the
LLM — designed to be staleness-safe:

- **No-tool replies only.** A turn that called any tool is never cached, so live
  account data (order status, balances…) is always fetched fresh.
- **Per-customer.** Keyed to the customer, so one customer's reply never leaks to
  another.
- **High similarity threshold** (`0.92` default) — only essentially-identical
  questions hit.
- **TTL-bounded** (`ttl_minutes`, default 60) — bounds staleness even for static
  answers.
- **Off by default.**

```yaml
cache:
  enabled: false
  similarity_threshold: 0.92
  ttl_minutes: 60
```

Stored in a `response_cache` table; a hit bumps a `hits` counter (observable).

## 10. Still deferred

- A global (cross-customer) FAQ cache and a real embedding model for the cache —
  the current cache uses the same lightweight embedding as memory, so it matches
  near-identical wording rather than deep paraphrases.

---

**End of daemon spec.**
