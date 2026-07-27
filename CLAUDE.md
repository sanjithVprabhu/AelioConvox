# Aelio — Claude Code Handoff (use as repo `CLAUDE.md`)

You are implementing **Aelio**, an agent operating system, from a completed normative specification: `AELIO_DSL_MOTHER.md` (in repo root). Read this file fully before any other action.

## Authority

1. **The mother document is the sole authority.** On any conflict between the doc and your instincts, training priors, or common patterns: the doc wins. It encodes ~25 deliberated decisions, 3 formal amendments, and 2 spec refinements — apparent "mistakes" are usually deliberate; check the Decision Log (Appendix D) for rationale before assuming error.
2. **Never weaken a locked invariant to make code compile or a test pass.** If an invariant seems unimplementable, that is a *finding* — raise it (see Flags below), do not route around it.
3. **Amendments are legal but never silent.** Any change to a locked section requires a Decision Log entry: date, section, change, rationale. Propose it as a flag first.

## Flags (how you raise findings)

Maintain `FLAGS.md` in repo root. When anything requires interpretation, seems contradictory, or breaks against reality (library limits, async lifetimes, Sunjet behavior), append an entry: `{id, section(s), what you found, options you see, your recommendation}`. **Flags are findings, not failures** — the spec's authors expect amendments #4+ to come from implementation. Do not block on a flag unless it gates correctness; otherwise take your recommended option, mark it `PROVISIONAL` in code comments, and continue.

## Phase 0 — Onboarding (before any code)

1. Read `AELIO_DSL_MOTHER.md` end to end.
2. Produce `docs/READBACK.md`: for each Part, 3–5 sentences of what it mandates, in your own words, plus anything that surprised you. This is a comprehension check, not summary theater — surprises are the point.
3. Open `FLAGS.md` with anything already unclear.

## Phase 1 — Generate the remaining annexes (F2–F6, F10–F11, F14)

Follow **Appendix F.1** exactly — each item lists sources, output format, and a completion check. Write each artifact to `docs/annexes/F<N>_<name>.md`. Run the completion check yourself and state pass/fail at the bottom of each file. These are pure derivation: if you find yourself making a design decision, stop — that's a flag.

Order: F2 → F5 → F3 → F6 → F4 → F14 → F11 → F10 (dependency-light first; F11 needs F2/F3; F10 needs everything).

## Phase 2 — P0 implementation (§30 order, strictly)

Workspace per §29: `aelio-sol`, `aelio-kernel`, `aelio-convert`, `aelio-learn`, `aelio-query`, `aelio-prompt`, `aelio-store`, `aelio-cli`. Dependency direction strictly downward; `aelio-sol` has zero internal deps.

**Tests come from the doc, then code comes from the tests:**
- The §8 per-op entries + App E schemas + F11 vectors ARE the conformance suite. Implement the vector-runner harness first (`aelio-kernel/tests/conformance.rs` reading `docs/vectors/`), then make vectors pass one op at a time.
- Property tests (§27): termination under budget; replay determinism (execute → ledger → replay → `bag_hash` equality); §14 rule-set non-computation; §4.3 canonical-hash stability (round-trip + cross-platform float formatting).

P0 sequence:
1. **`aelio-sol`**: SolValue types, §6.1 path grammar (from F2), §4.3 canonical serialization + BLAKE3 hashing, §4.4 limits, structural imprints (§4.1.3). *Exit: canonical-hash conformance green, including the int-vs-float (`2` ≢ `2.0`) and NaN-rejection vectors.*
2. **Planner** (§29): parse App E JSON → op tree; every static check listed in §29(1). *Exit: every "plan-time reject" claim in the doc has a rejecting test.*
3. **Executor + ledger + replay**: §12 sequential walk; App G entries with hash chain; INJECT/VERIFY replay; §12.4 intent/dispatch/result protocol. *Exit: replay-determinism property test green; divergence hard-refuses.*
4. **Control ops** (§8, all 17) against conformance vectors; then Compute v0 (§9/F3).
5. **Park/resume**: App I continuation format, §8.4 resume sequence, termination turns. *Exit: park/resume vectors + continuation_hash verification green.*
6. **Once/CAS** (§8.4, §24), minimal `aelio-store` behind a trait (Sunjet backend + in-memory test double — the double exists for tests only, Sunjet is the store).
7. **Golden test: the App A login flow end-to-end** with stub tool/model targets. *This is P0's definition of done.*

P1 and P2 follow §30 after P0 is green. Do not start P1 early.

## Working rules

- Rust edition 2021+, workspace monorepo. Deny warnings in CI. `unsafe` requires a comment citing why no safe alternative exists.
- Commit per completed vector-group or annex, message citing sections: `feat(kernel): Loop semantics per §8.3, vectors loop_*`.
- When you need a constant the doc doesn't give, check §4.4/§16.4/§21/§33 first; if genuinely absent, flag it — do not invent silently.
- The `sense` subtree is runtime-owned (§8.4): the Planner must reject ops writing under it — write that test early, it's cheap and load-bearing.
- Ledger `seq` is per-instance, monotonic, gapless (App G). Gapless is a test, not a comment.

## Definition of done (P0)

All conformance vectors green · property tests green · login golden flow green with replay bit-identity (`bag_hash` match on re-run) · `aelio-cli replay <instance> <turn>` demo works · `FLAGS.md` triaged (every flag resolved or explicitly deferred with rationale) · `docs/annexes/` complete with self-run completion checks.
