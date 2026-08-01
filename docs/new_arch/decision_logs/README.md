# Decision-log artifacts

Files in this directory are timestamped observations, not mutable specifications. Older logs are
kept as regression history and may show behavior that was subsequently corrected.

The current reference run is:

- `conversation_1784790606138_812909.log`

It demonstrates:

- pure clause pre-gating with readable reasons;
- flow-first login activation;
- redacted phone and OTP handling;
- typed `NeedsRepair` for a wrong OTP;
- transition to `authenticated`;
- a declared ranked query executing at Tier 2;
- three clean observations promoting the read-only path; and
- the next identical situation retrieving the promoted path at Tier 0.

Regenerate a new reference from `aelio-os`:

```bash
cargo run -p aelio --example decision_log_conversation
```

Production turns can emit the same independently redacted format to stderr with
`AELIO_DECISION_LOG=1`.
