# Build one harness: steps → Sol → saved

## Intent

> Greet the user. If they said “help”, offer help; otherwise ask what they need.

## Sugar steps

| # | Step | Meaning |
|--:|------|---------|
| 1 | `greet` | Say hello |
| 2 | `gate` | Does utterance contain `"help"`? |
| 3a | `offer_help` | Offer assistance |
| 3b | `ask_need` | Ask what they need |

```text
greet → gate → (offer_help | ask_need)
```

## Saved as Sol?

**Yes.** Stored in `sol_harness_contracts` with `kind: "sol_harness"`.

Proof test:

```bash
cd aelio-os
cargo test -p aelio-kernel --test sol_harness_store_replay build_greet_harness_steps_saved_as_sol
```

## Final Sol contract

See [`library/greet_then_offer_help.json`](./library/greet_then_offer_help.json).
