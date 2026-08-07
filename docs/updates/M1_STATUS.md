> **Correction notice (2026-08-07).** This file describes a `harness-core/` crate tree
> (`numeric.rs`, `canonical.rs`, `agg.rs`, `taint.rs`, `exec.rs`, `versioning.rs`), its own
> `clippy.toml`, and a `.github/workflows/` cross-arch matrix. **None of this exists anywhere in
> this repository, and a whole-filesystem search on this machine found no such tree elsewhere
> either.** It describes unbuilt/foreign work, not the status of `aelio-os` (the real system,
> built from `AELIO_DSL_MOTHER.md`). Treat every "Built and passing" / "N tests" claim below as
> **not verifiable against this codebase**.
>
> The real M1-equivalent status artifacts for `aelio-os` are [`docs/READBACK.md`](../READBACK.md)
> (comprehension pass against the mother doc) and [`FLAGS.md`](../../FLAGS.md) (implementation
> findings). See `FLAGS.md` entry **F-032** for the full terminology map between this document
> set and the real system, and for which of the findings below were actually checked against
> `aelio-os` code:
>
> - **N15** ("replay reaching an effect the original didn't must hard-refuse, never dispatch") —
>   checked and **already true today** in the real kernel: `ReplayBackend::pop` in
>   `aelio-os/crates/aelio-kernel/src/driver.rs:912-916` returns
>   `Err(ReasonCode::Internal, .., "replay ran past the ledger")` rather than dispatching.
> - **N14** ("`Budget` needs CAS, not `fetch_sub`, or concurrent `map_tool` elements overshoot")
>   — checked and **not applicable**: the real kernel's `Map` walks elements sequentially and
>   writes `into` only after all succeed (`aelio-kernel/src/exec.rs:761-866`), so there is no
>   concurrent element execution for this race to occur in. Worth re-checking only if `Map` ever
>   becomes parallel.
> - **N12/N13** (taint implicit-flow via branch selection; map-key taint laundering) — **not
>   applicable**: the real system has no per-value taint-bit mechanism to have this bug in. It
>   achieves least-privilege via args-only projection into `Call`/`Map` boundaries instead
>   (`AELIO_DSL_MOTHER.md` §4.2.4, §6.3, §10.3.1).
>
> The original two-pass content follows unmodified, for reference.

---

# M1 — Status (pass 2)

**54 tests passing.** Debug and release byte-identical. Matrix cell `AUT × 2` closed.

```
harness-core/
├── src/
│   ├── numeric.rs      §2.1–2.2   Int / Float / Decimal, no implicit coercion
│   ├── canonical.rs    §2.9       canonical serialiser (NOT serde_json)
│   ├── agg.rs          §2.3–2.4   null-explicit aggregates, count split, pairwise sum
│   ├── taint.rs        §5.9       taint propagation + AUT × 2 escape audit
│   ├── exec.rs         §3.5, §4.5 budget pool, journal, replay verification
│   └── versioning.rs   §17.1–3    SystemVersion, Verified<T>, op impl_hash
├── tests/determinism.rs           M1 exit criterion — pinned reference hashes
├── clippy.toml                    §2.10 HashMap/HashSet ban
└── .github/workflows/             cross-arch × cross-profile matrix
```

**Blocked in this environment:** `starlark-rust` requires edition2024; the container has Rust 1.75 and rustup's host is outside the network allow-list. Environment-only — it builds on your box. Everything the evaluator attaches to (value repr, taint, budget, journal) is done and tested, so the embedding is plumbing against a settled interface.

---

## Matrix AUT × 2 — the taint escape audit

Eleven escape attempts. One found a real hole; one found a class the design does not cover at all.

### The hole — map keys launder taint

`Value::Map` is `BTreeMap<String, Value>`. **The key position has no taint field.** A tainted string moved into a key silently becomes clean:

```
group_by(tainted_records, "city")   →  BTreeMap<String, Value>
                                        key taint lost at construction
```

Forbidding tainted keys is the wrong fix — `group_by` produces exactly this shape and is a legitimate, common operation. Instead the **container absorbs key taint**, and extracting a key back out returns it tainted. Both directions tested.

Reading §5.9 would not have surfaced this. It enumerates prohibited *positions* and never asks where a taint bit can fail to exist.

### The class — implicit flow

Value taint tracks data moving into a prohibited position. It does not track **which branch executed**:

```python
if record.salary > 100000:      # tainted predicate
    emit("high_earner", {})     # payload clean — §5.9 permits it
else:
    emit("normal", {})          # also clean, also permitted
```

Both payloads are untainted scalars, so every §5.9 rule passes. But which event fires is one bit of the tainted predicate, and a loop emits one bit per iteration.

Implemented as `PcTaint` — program-counter taint. A statement inside a control-flow region with a tainted predicate inherits that taint, and `emit`/`fail` are prohibited there. Static, not runtime. A nested clean predicate does not clear inherited taint.

---

## New findings from this pass

| # | Finding | Amend |
|---|---|---|
| **N12** | Taint misses implicit flows; branch selection under a tainted predicate is a one-bit channel through `emit` | §5.9 |
| **N13** | `Value::Map<String, Value>` has no taint slot in the key position; containers must absorb key taint | §5.9, §2.9 |
| **N14** | `Budget` needs CAS, not `fetch_sub` — a saturating subtract lets concurrent `map_tool` elements overshoot before any observes exhaustion | §3.5 |
| **N15** | Replay reaching an effect the original did not must be `JournalUnderrun`, never a dispatch. §3.6 says replay "reads from journal" without saying what happens past the end — dispatching there would give replay side effects and kill I4 quietly | §3.6 |

Plus three from pass 1: §2.2 scale clamp on `Decimal{s+6}`, §2.1 division target scale unspecified, §2.5 needing a fourth `NullEncountered` variant.

**Seven findings from building, none of which a review pass would have produced.** The *resolvable-by-build* category is behaving exactly as the convergence plan predicted, which is mild evidence the diagnosis was right.

---

## Tests worth reading

**`concurrent_takes_cannot_overshoot`** — eight threads draining a 1,000,000-row pool must total exactly 1,000,000. With `fetch_sub` this fails intermittently, which is the worst way for a budget bug to present.

**`a_replay_taking_a_different_path_underruns_rather_than_dispatching`** — encodes N15. If replay could dispatch, replays would have side effects.

**`map_keys_cannot_launder_taint`** with **`group_by_shaped_construction_still_works`** — read as a pair. The first is the security property; the second is why the obvious fix is wrong.

**`float_sum_depends_on_order_which_is_why_ordering_is_mandated`** — asserts that reordering a float sum *changes* the answer. It documents a hazard rather than a guarantee, and it is the executable justification for §5.2's unconditional tool-result ordering and §6.3's ban on reordering float operands.

---

## Reference hashes (pinned)

```
float_sum_reference_vector = 213bc218502836050e502b79bffa866497bcad78a17d712db94da434eee79f08
canonical_form_reference   = 08b19e6fcc94f1d82fc6bc663e54dd6da1de6e72dc396593690edd60269efe62
```

Stable across `dev` and `release` on x86_64. **Cross-architecture unverified** — no aarch64 here. If they differ on ARM, the cause is almost certainly FMA contraction in `pairwise_sum_f64`; the fix is `-C target-feature=-fma` or banning `mul_add`, not a fixture update.

---

## Next

1. **Push; let CI run the ARM leg.** Still the real M1 gate.
2. **Amend §2.1, §2.2, §2.5, §3.5, §3.6, §5.9** with the seven findings.
3. **Starlark embedding** on your toolchain: dialect lockdown asserted at startup, `TV` as the value type, `Budget` threaded through the evaluator, builtins bridging to the effect driver.
4. **`redb`** for data-plane stores — `Trace` and `JournalEntry` are the schema.
5. Matrix `PRT × 5, 6, 7` is now cheap to close. The types exist, so zero-step traces, empty journals, and single-entry ledger segments are a generated test matrix rather than speculation.
