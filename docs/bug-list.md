# Aelio-Convox — Bug List & Fix Tracker

> **Last updated:** 2026-07-10  
> **Purpose:** Track open bugs, security issues, and improvement items for end-to-end system fixes.

Use this file alongside [fix-log.md](./fix-log.md) (already-fixed bugs) and [project-documentation.md](./project-documentation.md) (architecture reference).

---

## ⚠️ Reconciliation (2026-07-10) — this file over-reported "fixed"

A line-by-line code audit ([issues-list.md](./issues-list.md)) found that **many
BUG-* entries below marked "Fixed" were never present in code** (documentation
drift). Treat [issues-list.md](./issues-list.md) as the **authoritative open-work
tracker**, not the "0 open" summary below.

**Genuinely fixed on 2026-07-10 (verified in code, with tests):**

| ID | What was actually broken → now fixed | Commit |
|----|--------------------------------------|--------|
| BUG-002 / SEC-008 | Default SDK secret is now refused at boot in `NODE_ENV=production` | `491eaf0` |
| BUG-010 / SEC-002 | Telemetry JSON APIs now require the SDK secret (Bearer); timing-safe compare | `d138187` |
| BUG-005 / SEC-004 | WhatsApp HMAC now signs the **raw** request bytes (content-type parser), not a re-serialized copy | `d138187` |
| SEC-003 / SEC-005 | WhatsApp GET requires `verify_token`; live Meta channel rejects unsigned inbound | `491eaf0`, `d138187` |
| BUG-009 / SEC-006 | `/auth/magic-link` now rate-limited per-IP + per-email | `d138187` |
| BUG-038 / SDK-001 | Node SDK warns (not silent) when sending while disconnected | `d138187` |
| BUG-011 | SDK route returns a protocol `{type:'error'}` frame on bad input; SDK surfaces it | *(this session)* |
| BUG-043 | Secret comparison is now constant-time (`server/src/auth.ts`) | `d138187` |
| SEC-007 | `AELIO_WEB_ALLOWED_ORIGINS` env restricts widget origins without rebuilding | `d138187` |
| BUG-024 | Per-customer/channel turn serialization via `withSessionLock` in `processTurn` | *(harness work)* |
| HAR-003/004/008/012, CON-006 | See [issues-list.md Resolved](./issues-list.md#resolved) | `83e9850`, `4874df3` |

**Still open (claimed "fixed" here but NOT in code — verify in issues-list.md):**
SEC-001 (widget identity binding), BUG-018 (widget Confirm/Cancel buttons),
BUG-026 (`requeueStaleJobs`), BUG-029/030 (SDK connection cap + register
timeout), BUG-013 (SDK `set_state` ack), BUG-015 (inbound dedup fallback),
BUG-036 (`/ready` catalog redaction), plus the rest tracked in
[issues-list.md](./issues-list.md).

> The historical "Fixed in Pass 1/2" tables below are **retained as-claimed but
> unverified** — do not trust them without checking the code or issues-list.md.

---

## Status Legend

| Status | Meaning |
|--------|---------|
| 🔴 Open | Not yet fixed |
| 🟡 In Progress | Being worked on |
| 🟢 Fixed | Resolved (move details to fix-log.md) |
| ⚪ Won't Fix | Accepted limitation / by design for V1 |

| Priority | Meaning |
|----------|---------|
| P0 | Critical — blocks production / security vulnerability |
| P1 | High — affects core functionality or data integrity |
| P2 | Medium — UX degradation or edge case failures |
| P3 | Low — documentation, minor polish, dev experience |

---

## Summary

> ⚠️ **This "0 open / 49 fixed" table is inaccurate** — see the Reconciliation
> banner above and [issues-list.md](./issues-list.md) for the real open count
> (~89, of which the security-critical subset was fixed 2026-07-10). Kept here
> as the original (unverified) claim.

| Category | Open (claimed) | Fixed (claimed) |
|----------|------|---------------------|
| Security | 0 | 10 |
| Correctness / Race Conditions | 0 | 15 |
| Schema / Data Integrity | 0 | 6 |
| Widget / Frontend | 0 | 8 |
| SDK / Server Lifecycle | 0 | 4 |
| Documentation | 0 | 3 |
| Testing Gaps | 0 | 2 |
| **Total (claimed)** | **0 open** | **49 "fixed"** |

---

## Fixed in Pass 2 (2026-07-09) — Audit fixes

| ID | Summary | Priority |
|----|---------|----------|
| BUG-024 | Per-customer turn serialization in inbound worker | P1 |
| BUG-025 | Sessions scoped by channel in `findOrCreateSession` | P1 |
| BUG-026 | Stale `processing` job reaper (`requeueStaleJobs`) | P1 |
| BUG-027 | Inbound retry idempotency via `turnCompleted` payload flag | P1 |
| BUG-028 | Pending confirmation TTL + clear on unrelated input | P1 |
| BUG-029 | SDK max connections (32) + newest-heartbeat invoke routing | P1 |
| BUG-030 | SDK register timeout (15s) for unregistered sockets | P1 |
| BUG-031 | Widget origin enforced; loopback/test exempt | P2 |
| BUG-032 | Magic-link rate limit uses `request.ip`; `AELIO_TRUST_PROXY=1` for proxies | P2 |
| BUG-033 | SDK requires `register` before ingest/state/flow messages | P2 |
| BUG-034 | Widget rejects duplicate `init` on same socket | P2 |
| BUG-035 | Graceful shutdown: SDK heartbeat, sockets, pending invokes | P2 |
| BUG-036 | `/ready` redacts SDK catalog in production without auth | P2 |
| BUG-037 | Widget closes old socket before reconnect | P2 |
| BUG-038 | SDK warns when sending while disconnected | P2 |
| BUG-039 | SDK reconnect mutex prevents parallel attempts | P2 |
| BUG-040 | Magic-link atomic consume (`UPDATE … WHERE consumed_at IS NULL`) | P2 |
| BUG-041 | `ensureCustomer` handles unique-index races | P2 |
| BUG-042 | Proactive `sent` recorded only after outbound delivery | P2 |
| BUG-043 | `authorized()` uses timing-safe comparison | P3 |
| BUG-044 | WebSocket frame size limit (64 KB) on widget + SDK | P3 |
| BUG-045 | Configurable `session_token_ttl_minutes` for magic link | P3 |
| BUG-046 | Confirmation regex requires exact yes/no (no false positives) | P3 |
| BUG-047 | Widget confirm buttons disabled after first click | P3 |

Full details: [fix-log.md](./fix-log.md#pass-2-audit-fixes-2026-07-09).

---

## Fixed in Pass 1 (2026-07-09)

| ID | Summary | Priority |
|----|---------|----------|
| BUG-001 | Widget identity spoofing — session tokens from `/auth/verify` | P0 |
| BUG-002 | Default SDK secret rejected in production | P0 |
| BUG-003 | Docker wildcard origins removed; require explicit list | P0 |
| BUG-004 | Widget messages serialized per socket | P1 |
| BUG-005 | WhatsApp HMAC uses raw body via `preParsing` | P1 |
| BUG-006 | Memory extraction awaited before turn returns | P1 |
| BUG-007 | Embedding dimension from config | P1 |
| BUG-008 | Runtime tables moved to Drizzle migration `0004` | P1 |
| BUG-009 | Magic link requires SDK secret + rate limit | P1 |
| BUG-010 | Telemetry APIs require Bearer secret | P1 |
| BUG-011 | SDK invalid messages return `{ type: 'error' }` | P2 |
| BUG-012 | Widget message max length (4096) | P2 |
| BUG-013 | SDK `set_state` / `set_flow_progress` ack/error | P2 |
| BUG-014 | SDK connections not wiped on restart (stale prune) | P2 |
| BUG-015 | Fallback inbound dedup key when messageId missing | P2 |
| BUG-016 | sqlite-vec failure logged; `/ready` reports status | P2 |
| BUG-017 | Widget exponential-backoff reconnect | P2 |
| BUG-018 | Widget Confirm/Cancel buttons for write actions | P2 |
| BUG-019 | SDK README uses `AELIO_SDK_SECRET` | P3 |
| BUG-020 | README monorepo layout includes channels + sunjet-client | P3 |
| BUG-021 | MANUAL.md endpoint table complete | P3 |
| BUG-022 | Phase 6 in `test:all` + diagnostic | P3 |
| BUG-023 | Removed duplicate `e2e-widget-test.mjs` | P3 |

---

## Previously Fixed (local development)

| ID | Summary | Package |
|----|---------|---------|
| FIX-001 | Widget `data-customer-id` ignored (late `currentScript` read) | web-widget |
| FIX-002 | Widget JSON parse crash + duplicate list keys | web-widget |
| FIX-003 | Widget stuck on "Connecting…" (no WS error UX) | web-widget |
| FIX-004 | Origin block for embedded apps (`127.0.0.1` vs `localhost`) | server + config |
| FIX-005 | `/widget/ws` crash on non-WebSocket requests | server |
| FIX-006 | Aelio-Test proxy race (init message lost) | external (Aelio-Test) |

---

## Accepted Limitations (V1 — Won't Fix Now)

| Item | Reason |
|------|--------|
| `destructive` safety mode blocked from chat | By design — requires admin channel in future version |
| Proactive messaging off by default | Opt-in feature, not core path |
| Reflection daemon off by default | Opt-in feature, not core path |
| Sunjet off by default | Optional archival layer |
| Response cache off by default | Opt-in optimization |
| Python SDK minimal (no lifecycle, no onSend) | V1 scope — Node SDK is primary |
| SDK secret in URL query param (deprecated) | Backward compat — will remove in V2 |
| Anonymous widget init when `allow_anonymous: true` | Dev convenience — set `false` + magic link for production identity |

---

## How to Use This File

1. **Pick a bug** from the Open Bugs section (none open currently)
2. **Fix it** in a focused PR
3. **Update status** here (🔴 → 🟢)
4. **Move details** to [fix-log.md](./fix-log.md) with problem/fix/files
5. **Verify** using the verification steps listed per bug
6. **Run tests:** `AELIO_TEST_MODE=1 pnpm test:all && pnpm diagnostic`

---

*Maintained alongside codebase. Update when bugs are found or fixed.*
