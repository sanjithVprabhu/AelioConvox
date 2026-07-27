# F2 — Path grammar EBNF (§6.1)

**Sources:** §6.1, §4.2.5, §4.4.  
**Status:** pure elaboration of locked text. No design decisions.

## EBNF

```ebnf
path     = segment ( "." segment | index )*
segment  = identifier | quoted_key
index    = "[" integer "]"
identifier = letter_or_us ( letter_or_us | digit )*
quoted_key = "[" '"' ( char_no_quote | escape )* '"' "]"
letter_or_us = "A"…"Z" | "a"…"z" | "_"
digit    = "0"…"9"
integer  = digit+                    (* no leading zeros required by §6.1; empty forbidden *)
escape   = "\\" ( '"' | "\\" | "/" | "b" | "f" | "n" | "r" | "t" | "u" hex{4} )
```

**Normative constraints (not expressible in pure EBNF but enforced by the parser):**

1. Path depth ≤ **32** segments (§4.4). Depth >32 ⇒ reject.
2. First element must be a **segment** (key), never an `index` — bag root is a map.
3. After a segment, a quoted key may only appear after `.` (grammar: `.` segment). A bare `["k"]` following a segment without `.` is a syntax error.
4. Paths in instructions are **literals only** (§4.2.5). There is no production for variables, expressions, or computed segments — the grammar structurally cannot express them.
5. **Root `into` is illegal** (§4.2.4): a path must have ≥1 segment. Empty string / `.` alone are rejected. The whole bag is not a legal write target for `Call`/`into`.

## Positive examples (≥10)

| # | Path | Notes |
|---|------|--------|
| 1 | `auth_raw` | single identifier |
| 2 | `auth_raw.loggedin` | dotted keys |
| 3 | `phone.e164` | App A |
| 4 | `reply1.text` | App A Park into |
| 5 | `otp_send` | Call into |
| 6 | `hydration_meta` | App A |
| 7 | `items[0]` | index form |
| 8 | `items[0].name` | index then key |
| 9 | `["user-id"]` | quoted non-identifier key |
| 10 | `meta.["user-id"].tags[2]` | mixed |
| 11 | `a.b.c.d.e.f.g.h.i.j.k.l.m.n.o.p.q.r.s.t.u.v.w.x.y.z.aa.ab.ac.ad.ae.af` | depth 32 keys (boundary OK) |
| 12 | `_private.x` | leading underscore |

## Negative examples (≥10)

| # | Input | Why reject |
|---|--------|------------|
| 1 | `` (empty) | no segment; root into illegal |
| 2 | `.foo` | leading dot |
| 3 | `foo.` | trailing dot |
| 4 | `0abc` | identifier cannot start with digit |
| 5 | `foo-bar` | bare hyphen not in identifier |
| 6 | `[0]` | first element cannot be index |
| 7 | `foo["bar"]` | quoted key needs leading `.` |
| 8 | `foo..bar` | empty segment |
| 9 | `foo[01]` | (if leading-zero policy strict) or invalid trailing |
| 10 | `foo[ ]` | empty index |
| 11 | `foo.bar.baz` × depth 33 | Limit.Depth |
| 12 | `${x}.y` / `foo[i]` | computed path — no grammar production |

## Paths appearing in §8 / App A / App E that must parse

`hydration_meta`, `auth_raw`, `auth_raw.loggedin`, `r`, `ask1`, `reply1`, `reply1.text`, `phone`, `phone.e164`, `otp_send`, `ask2`, `reply2`, `reply2.code`, `verify`, `sw`, `out`, `sense.env.now` (read; write under `sense` is plan-time **Policy** reject, not path-syntax reject), `err_into` targets, Map `over`/`into` paths.

## Completion check

| Check | Result |
|-------|--------|
| Every path in §8, App A, App E parses under this grammar | **PASS** (identifiers + indices + quoted keys cover them) |
| Grammar cannot express computed paths | **PASS** (no non-literal productions) |
| Root `into` impossible (≥1 segment required) | **PASS** |
| Depth >32 rejected | **PASS** (constraint + `aelio-sol` tests) |
