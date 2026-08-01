# Fix proposal: Wrong OTP yields `SigMismatch` instead of `NeedsRepair`

Status: **proposed, not implemented**
Observed in: simulation 6 (`docs/new_arch/AELIO_RUST_12_SIMULATIONS.md`)
Primary code: `aelio-os/crates/aelio/src/abilities/invoke.rs`

## Problem

`verify_otp` declares:

1. **Success shape** — required field `otp_verified` at path `ok` (bool)
2. **Error taxonomy** — `invalid_otp` → `needs_repair` / recovery `re-ask`

When the tool returns `{ "error": "invalid_otp" }`, the intended outcome is:

```text
ResponseRole::Error → ReasonCode::NeedsRepair → stay on await_otp → re-ask OTP
```

Actual outcome today:

```text
structural_interpret requires `ok` → ReasonCode::SigMismatch
(error classification never runs)
```

## Root cause

In `tool_call_block` / invoke path, order is:

1. Call tool
2. `structural_interpret(raw)` against **success** `output_semantics.fields`
3. Then `sig::classify` → error role → `classify_error`

Step 2 fails on error payloads that omit success fields, so step 3 is unreachable.

## Recommended fix (preferred)

**Classify response role before requiring success-field extraction.**

Pseudo-order:

```text
raw = Invoke.Call(...)
shape/hash = Sig.Compute(raw)          # still on raw

role = Sig.Classify(raw, role_hint)
if role == Error:
    (code, recovery) = classify_error(raw, tool.errors)
    return Err(NeedsRepair|...) with sanitized detail
    # do NOT require success fields

# success / continuation / effect_confirmation only:
extracted = match_plan OR structural_interpret(raw, success fields)
sanitize + return InvokeReceipt
```

### Why this is right

- Success fields describe successful responses, not error envelopes.
- Error taxonomy already exists on `ToolSpec.errors` for this case.
- Flow step `on_violation: Repair` can only fire if the reason is `NeedsRepair`, not `SigMismatch`.
- Matches the architecture rule: errors are typed outcomes with recovery, not shape failures.

### Acceptance criteria

1. Wrong OTP returns `ReasonCode::NeedsRepair` (or declared recovery), never `SigMismatch` solely because `ok` is absent.
2. Correct OTP still extracts `otp_verified` from `ok: true`.
3. Truly malformed success payloads (missing `ok` when role is Data/EffectConfirmation) still yield `SigMismatch`.
4. Simulation 6 assertion passes; no PII/OTP leaked in durable error details.
5. Golden / durable OTP tests still pass.

## Alternative (narrower, acceptable)

Keep current order but make `structural_interpret` skip required success fields when `sig::classify` would mark Error — effectively the same rule, just inlined earlier via a role peek.

Prefer the explicit reorder for clarity and auditability.

## Non-goals

- Do not invent OTP values or guess on mismatch.
- Do not treat all missing fields as repair; only declared error roles/taxonomy.
- Do not weaken success-shape validation for non-error responses.

## Follow-up after fix

Re-run:

```bash
cargo run --manifest-path aelio-os/Cargo.toml -p aelio --example aelio_12_simulations
```

Expect scenario 6: `supported` / assertion pass, gap text removed.
