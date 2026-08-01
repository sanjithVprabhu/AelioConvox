# AELIO RENDER PROTOCOL & CHAT SDK — TECHNICAL SPECIFICATION

**Version:** 1.0
**Status:** NORMATIVE for implementation. Companion to `TECHNICAL_MOTHER_SPECIFICATION.md` ("the harness spec") and governed by `AELIO_DSL_MOTHER.md` ("the mother doc"). This document owns everything between the Aelio server boundary and the user's screen. It restates nothing from its companions; it references them.
**Audience:** the SDK engineer and the server-side engineer of the render seam.

**Conventions.** `MUST`/`MUST NOT`/`MAY` normative · `id@vN` pinned reference · **[R-n]** = ratified decision, indexed in §14 · imprint/glu/reaction/trust vocabulary per the harness spec §1.

---

## 0. Thesis

> **The frontend is glu pointed at the human.** [R-1]

The chat widget is a *consumer* with declared input shapes; the server is a *producer* of typed outputs. The screen is therefore a seam, and it gets seam discipline: declared imprints on both sides, validate–convert–validate, closed vocabularies, versioned everything. Nothing crossing in either direction is ever code, markup, or free-form structure.

Two flows, one protocol:

```text
server → client : RenderFrame   (what to show)      — "the down seam"
client → server : EventFrame    (what the user did) — "the up seam"
```

Both directions are validated at both ends. The model fills closed schemas (harness spec D-7); the SDK's registered renderers are the only code that touches the DOM; user interactions arrive as typed events, never free-form payloads. The injection-containment story of the backend extends to the browser **iff** every rule in §9 holds.

### 0.1 Guarantees

- **F1 — No code crosses the wire.** RenderFrames carry declarative blocks only. No HTML, no JS, no CSS, no URLs interpreted as code. (Mother §4.1.4 extended to the render seam.)
- **F2 — Closed kinds, both directions.** Every block `kind` and every event `type` belongs to a versioned, registered, closed set. Unknown = fallback (down) or reject (up), never improvise.
- **F3 — Capability-scoped rendering.** The server renders only into the kind-set the connected client declared at handshake. Skew degrades gracefully by protocol rule, not SDK courtesy.
- **F4 — Typed interactivity.** An event may only carry values validating against the emitting block's declared event imprint, and may only trigger what that block's `on` binding names — which is always a pinned, promoted artifact.
- **F5 — Replayable screens.** Every RenderFrame and EventFrame is ledgered as a reaction; replay reconstructs exactly what the user saw and did.

---

## 1. Vocabulary

| Term | Definition |
|---|---|
| **Session** | One connected widget instance: handshake → frames → close. Carries `session_id`, tenant, principal. |
| **Frame** | One protocol message. `RenderFrame` (down) or `EventFrame` (up). |
| **Block** | One renderable unit inside a RenderFrame: `{block_id, kind@v, body, fallback, on?}`. |
| **Element** | The SDK-side registered renderer for one `kind@v`: body imprint + renderer + (optional) event imprint. |
| **Kind** | The registered identity of an element (`chart@2`). Closed set per SDK build. |
| **Binding (`on`)** | A block's declared map from event type → server-side pinned target. |
| **Patch** | A RenderFrame that updates existing blocks by `block_id` instead of appending. |
| **Capability set** | The `{protocol version, kinds[], limits}` a client declares at handshake. |

---

## 2. Session lifecycle

```text
1  client → server : Hello        {protocol:"aelio-render@v1", kinds:[...], limits:{...}, resume?:session_id}
2  server → client : Welcome      {session_id, accepted_kinds:[...], server_limits:{...}}
3  loop:
     client → server : EventFrame (user turn: message | interaction event)
     server → client : RenderFrame* (one or more; streaming §7)
4  either side     : Close        {reason_code}
```

- The server's `accepted_kinds` is the **intersection** of what the client offered and what the server supports. Both sides hold the same set for the session's lifetime; capability changes require a new session. [R-2]
- `resume` reattaches to a live session after a transport drop; the server replays undelivered frames from the ledger (F5), which is why resume needs no bespoke state machine. [R-3]
- All frames ride one ordered, reliable transport (WebSocket or equivalent). Frame order is semantic: blocks render in arrival order; patches apply in arrival order.

---

## 3. RenderFrame — the down seam

```json
{"frame":"render", "frame_id":"string (monotonic per session)",
 "mode":"append|patch|replace_turn",
 "turn_id":"string",
 "blocks":[Block, ...]}

Block = {"block_id!":"string (unique within turn)",
         "kind!":"<kind>@v<n>",
         "body!":"object (validates against the kind's body imprint)",
         "fallback!":"Block with kind text@1 (REQUIRED unless kind IS text@1)",
         "on":{"<event_type>":"<pinned server target id@v>"},
         "meta":{"width":"auto|full", "collapsed":"bool"}}
```

Rules:
- **`fallback` is mandatory** on every non-text block [R-4]. A client that cannot render `kind` (skew, load failure, render error) MUST render the fallback and MUST emit a `render_degraded` event. The server thereby *always* knows what the user actually saw.
- `mode:"append"` adds blocks to the current turn. `"patch"` replaces the `body` of existing `block_id`s (for live-updating charts, progress, streaming text). `"replace_turn"` atomically swaps the whole turn's blocks (rare; error recovery).
- A patch to an unknown `block_id` is a client-side protocol fault: the client emits `patch_orphan` and ignores it. The server MUST NOT assume a patch applied until unacknowledged-fault timeout passes. [R-5]
- `on` bindings may only name **pinned, promoted** server targets (harness spec §16). A binding to anything else fails server-side validation before the frame is ever sent — glu outbound on the down seam. [R-6]

### 3.1 Who authors RenderFrames

A synthesis prompt whose output imprint IS the frame's block list (harness spec D-7), or deterministic server code, or a mix. In all cases the frame validates against `render_frame@v1` **server-side before send** (glu outbound) and **client-side on receipt** (glu inbound). Client-side validation failure of a single block renders that block's fallback, not a blank screen. [R-7]

---

## 4. Core element registry (SDK v1)

The closed kind set shipped in v1. Body imprints are normative in Appendix A; this table is the contract summary. Every element also accepts `meta`.

| Kind | Renders | Body (summary) | Emits (event types) |
|---|---|---|---|
| `text@1` | Markdown subset (see §9.2) | `{md}` | — |
| `code@1` | Syntax-highlighted, copy button | `{lang, source}` | `copied` |
| `table@1` | Sortable table, client-side only | `{columns:[{key,label,type}], rows:[[...]]}` | `row_selected {row_index}` |
| `chart@1` | line/bar/area/pie/scatter | `{type, series:[{name, points}], axes:{x,y}}` | `point_selected {series, index}` |
| `diagram@1` | DAG/flowchart from declarative nodes+edges | `{nodes:[{id,label,shape}], edges:[{from,to,label?}]}` | `node_selected {node_id}` |
| `metric@1` | KPI card(s) | `{items:[{label, value, delta?, unit?}]}` | — |
| `form@1` | Typed input group + submit | `{fields:[FormField], submit_label}` | `submitted {values}` |
| `choice@1` | Buttons / chips (one-shot) | `{options:[{id,label,style?}], multi:bool}` | `chosen {ids}` |
| `confirm@1` | Explicit yes/no with consequence text | `{prompt, yes_label, no_label, danger:bool}` | `confirmed {answer}` |
| `status@1` | Progress/spinner/step list (patch target) | `{state:"running|done|error", label, steps?}` | — |
| `media@1` | Image via handle reference only | `{handle, alt, caption?}` | — |
| `group@1` | Vertical/horizontal container of Blocks | `{direction, blocks:[Block]}` | — |

`FormField = {name, label, type:"string|number|bool|enum|date", required, enum_values?, placeholder?}` — field `type`s are the closed set; `form@1` values validate client-side against these AND server-side against the binding target's input imprint (double validation, glu at both ends).

Registry rules:
- Adding a kind or bumping a kind's body imprint = new SDK minor version; removing = major. Kinds are artifacts in the bank (`class:"element"`), versioned like everything else. [R-8]
- `group@1` nests; nesting depth ≤ 4, total blocks per frame ≤ 64, patch rate ≤ 10/s per block — protocol limits declared in the handshake and enforced by BOTH sides (App B constants). [R-9]
- `media@1` takes a **handle** (mother §4.1 two-tier), resolved by the client against the Aelio content endpoint with the session's auth — never a raw URL to arbitrary origins. [R-10]

---

## 5. EventFrame — the up seam

```json
{"frame":"event", "frame_id":"string (monotonic per session)",
 "turn_id":"string",
 "event":
   {"type!":"message|copied|row_selected|point_selected|node_selected|submitted|chosen|confirmed|render_degraded|patch_orphan",
    "block_id":"string (absent only for type=message)",
    "value":"object (validates against the emitting kind's event imprint)"}}
```

- `message` is the ordinary chat turn: `{value:{text}}` — the one free-text field on the up seam, treated as data everywhere (never parsed as instructions by any `k.` step; rendered into prompts under the same rules as any untrusted input).
- **Every non-message event MUST reference a live `block_id` whose kind declares that event type, and its `value` MUST validate against that kind's event imprint.** Anything else is rejected server-side with `event_invalid` — glu inbound on the up seam. A client cannot invent capabilities; a tampered client cannot widen the surface. [R-11]
- Events on blocks with an `on` binding route to the bound pinned target, invoked through the normal reactor path (policy gates, effects, budget, ledger — an event is just an input arriving at a harness). Events without a binding are recorded (analytics/context) and trigger nothing. **The event's `value` is the ONLY payload the bound target receives from the client** — least privilege on the up seam. [R-12]
- `confirm@1` is the designated consent element: any bound target whose effect union includes `write` or `external` MUST be reachable only via a `confirmed{answer:true}` event — enforced server-side at binding-validation time, i.e. a frame binding a `write` target to a bare button is invalid *before it is sent*. [R-13]

---

## 6. Turn semantics

A **turn** = one user-visible exchange: opened by an EventFrame of type `message` (or an event on a bound block), closed when the server sends `status` done or the next turn opens. `turn_id` scopes `block_id` uniqueness and `replace_turn`. Events referencing blocks from earlier turns remain valid while those blocks are on screen (tables stay clickable) — the server resolves the binding from the ledgered frame, not from session memory. [R-14]

## 7. Streaming and progressive rendering

- Long text: send `text` block once, then `patch` its `md` with grown content. The patch stream is ordered; the client renders monotonically.
- Long operations: send `status@1` immediately, patch its steps, patch to `done|error` at close. The harness spec's demand loop (harness spec §12) surfaces here: a capability miss renders an honest `status:error` + `text` explanation — the "can't do that yet" made visible.
- The server MUST close every turn with a terminal state (`status` done/error or final append); a turn without a terminal state after `turn_timeout` (App B) is closed client-side with a degraded notice. [R-15]

## 8. Validation summary — glu at four points

```text
DOWN: server: frame validates against render_frame@v1 + per-kind body imprints  (before send)
      client: same validation on receipt; per-block failure → fallback + render_degraded
UP:   client: event validates against emitting kind's event imprint (before send)
      server: same validation + block/binding/liveness checks; failure → event_invalid
```

Neither end ever trusts the other's validation. Both ends run from the same versioned imprints, shipped in the SDK package and registered in the bank — one source of truth, two enforcement points. [R-16]

## 9. Security containment

### 9.1 Down-seam rules
1. No block body field is ever interpreted as HTML/JS/CSS. `text@1` renders a **Markdown subset** (§9.2); everything else renders through typed component props. String → DOM text node, always.
2. `media@1` handles resolve only against the Aelio content endpoint (R-10). No external origins, no data URIs.
3. `on` bindings: pinned + promoted only (R-6); consent gating for effectful targets (R-13).

### 9.2 The Markdown subset (`text@1`)
Permitted: paragraphs, bold/italic/strikethrough, inline code, fenced code, lists, blockquotes, headings h1–h4, tables, horizontal rules. **Forbidden: raw HTML passthrough, images, links with schemes other than `https`, autolinked scripts.** Links render with forced `rel="noopener noreferrer"` and a visible domain. The subset is a registered artifact (`sdk.md_subset@v1`) so tightening it is a version bump, not a silent change. [R-17]

### 9.3 Up-seam rules
1. Closed event vocabulary; imprint-validated values; liveness + declared-emitter checks (R-11).
2. Rate limits per session and per block (App B); excess → `rate_limited` close.
3. The bound target receives only `event.value` (R-12) — never session cookies, never DOM state, never other blocks' contents.
4. `message` text is data. It flows into prompts under the mother doc's untrusted-input discipline; nothing on the server ever executes it.

### 9.4 What a fully compromised client can achieve
Stated so the bound is explicit: send valid events on live blocks it was shown, at capped rate, with imprint-valid values, reaching only promoted targets it was bound to, gated by consent for effects. It cannot: render anything (server ignores unknown frames), reach unbound targets, exceed declared value shapes, or widen effects. The blast radius is "a user clicking things," which is the correct bound. [R-18]

## 10. Version skew

- Protocol version in `Hello`; unknown major → refuse session with upgrade notice.
- Kind-level skew handled by intersection (R-2) + mandatory fallback (R-4). The server MAY consult `accepted_kinds` when *choosing* how to synthesize (don't plan a diagram for a client that can't draw one) — the capability set is a legal input to the synthesis prompt's slots. [R-19]
- SDK builds pin their element registry versions; a session's semantics are fully determined by `(protocol@v, accepted_kinds@v)` — both ledgered in `Welcome`, so replay renders identically. 

## 11. Ledger and replay

`Hello`/`Welcome`, every RenderFrame, every EventFrame = one reaction each in the session's ledger stream (harness spec A.8; `kind` extended with `render|event|handshake` for this seam — an extension flagged for the harness spec's next revision). Replay of a session reproduces the exact screen sequence and the exact interaction sequence. Frames are content-hashed; `frame_id` monotonicity makes gaps detectable, which is what `resume` (R-3) is built on.

## 12. SDK package shape

```text
@aelio/chat-sdk
  /core        transport, handshake, frame codec, validators (imprint-driven, generated)
  /elements    one module per kind@v (renderer + body imprint + event imprint)
  /theme       tokens only — hosts restyle via tokens, never by injecting markup
  /index       <AelioChat session={...} />  — the one public component
```

- Validators are **generated from the bank's imprints** at SDK build time — the SDK cannot drift from the server's contracts without failing its own build. [R-20]
- Host apps extend by *registering additional elements* (own kind namespace `x.<org>.<name>@v`), declared in `Hello` like any kind; they cannot override core kinds.

## 13. Reason codes (closed, this protocol)

```text
handshake_version_unsupported  kind_set_empty  frame_malformed  block_body_invalid
fallback_missing  binding_unpinned  binding_unpromoted  consent_required
patch_orphan  render_degraded  event_invalid  event_orphan_block  event_undeclared_type
rate_limited  turn_timeout  session_resumed  session_closed
```

## 14. Decision log

| # | Decision |
|---|---|
| R-1 | The frontend is glu pointed at the human; screen = seam |
| R-2 | Capability set = handshake intersection, fixed per session |
| R-3 | Resume = ledger replay of undelivered frames |
| R-4 | Mandatory fallback block on every non-text block |
| R-5 | Orphan patches ignored + faulted, never guessed |
| R-6 | Bindings name pinned + promoted targets only |
| R-7 | Per-block client validation failure → fallback, not blank screen |
| R-8 | Elements are versioned bank artifacts; kind changes = SDK version bumps |
| R-9 | Structural limits (nesting 4, blocks 64, patch 10/s) in the handshake, enforced both ends |
| R-10 | Media by handle via Aelio content endpoint only |
| R-11 | Events: closed vocabulary, live block, declared emitter, imprint-valid value |
| R-12 | Bound target receives event.value only |
| R-13 | write/external targets reachable only through confirm@1 |
| R-14 | Bindings resolve from ledgered frames, not session memory |
| R-15 | Every turn ends in a terminal state |
| R-16 | Dual validation from one imprint source at all four seam points |
| R-17 | Markdown subset is a versioned artifact |
| R-18 | Compromised-client blast radius = "a user clicking things" |
| R-19 | accepted_kinds is a legal synthesis input |
| R-20 | SDK validators generated from bank imprints at build time |

---

## Appendix A — Protocol imprints

`render_frame@v1`, `event_frame@v1`, `hello@v1`, `welcome@v1`, `block@v1` exactly as shaped in §§2–5; per-kind body and event imprints as summarized in §4, normative in the bank under `element.<kind>@v`. (Schemas follow harness spec Appendix A notation; `!` = required; PIN rules identical.)

## Appendix B — Constants (shipped defaults, versioned in `sdk.config@v1`)

| Constant | Default |
|---|---|
| max nesting depth / blocks per frame | 4 / 64 |
| patch rate per block / events per session | 10/s / 5/s sustained, burst 20 |
| turn_timeout | 120 s |
| resume window | 15 min |
| md subset | sdk.md_subset@v1 |
| handshake kinds max | 128 |

— end of specification —
