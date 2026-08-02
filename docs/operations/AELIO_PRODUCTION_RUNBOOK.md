# Aelio production runbook

This runbook applies to the unified deployment: one authoritative Rust `aelio-server` owns the
kernel, adaptive runtime, artifact authority, Aelio DB, ledgers and continuations; TypeScript is the
authenticated model/embedding/SDK/channel host edge.

## Startup contract

Production must provide all of the following. Do not set `AELIO_ALLOW_INSECURE_OPEN`.

- `AELIO_DATA_DIR`: durable local volume containing `runtime/`, `database/`, and `agent/`.
- `AELIO_RUNTIME_TOKENS`: one or more comma-separated bearer tokens, each at least 16 characters.
- `AELIO_EVENT_KEY_SECRET`: at least 32 bytes, stable across restarts.
- `AELIO_HOST_URL` and `AELIO_HOST_TOKEN`: authenticated Rust-to-TypeScript reverse boundary.
- `AELIO_LLM_GATEWAY_URL` and an embedding endpoint/provider through the TypeScript edge.

Keep the event-key secret and internal tokens outside the data backup and restore them from the
deployment secret manager. Rotating the event-key secret invalidates outstanding event wakes.

`GET /healthz` proves the process is alive. `GET /readyz` proves authentication is configured and
the runtime artifact store can be opened with its current schema. Send traffic only when ready.

## Graceful drain and shutdown

Send `SIGTERM` (containers/orchestrators) or `SIGINT`. The Rust listener stops admitting new HTTP
work and Axum drains admitted requests before the process exits. The TypeScript edge also closes
its listener, workers, correlated SDK invocations, and WebSockets. Configure the orchestrator's
termination grace period above the largest admitted request deadline (normally at least 35s).

Never copy a live data directory. After both processes exit cleanly, take an offline backup.

## Backup

The Rust command snapshots the entire data root, rejects symlinks/nested destinations, hashes every
file with BLAKE3, fsyncs copied files and writes a closed `aelio-backup/1` manifest.

```bash
cargo run --manifest-path aelio-os/Cargo.toml -p aelio-server -- \
  backup /var/lib/aelio /var/backups/aelio/2026-08-01

cargo run --manifest-path aelio-os/Cargo.toml -p aelio-server -- \
  verify-backup /var/backups/aelio/2026-08-01
```

Copy the completed backup to separate failure-domain storage only after verification succeeds.
Retention and encryption are operator responsibilities; the snapshot contains customer data.

## Restore

Restore is deliberately non-overwriting. The target must not exist, which prevents an operator
from destroying the only recoverable copy. The command verifies the manifest before copying and
atomically renames a sibling staging directory into place.

```bash
cargo run --manifest-path aelio-os/Cargo.toml -p aelio-server -- \
  restore /var/backups/aelio/2026-08-01 /var/lib/aelio-restored
```

Point `AELIO_DATA_DIR` at the restored directory, supply the original event-key secret, start Rust,
wait for `/readyz`, then start the TypeScript edge. Run a read-only turn and inspect its decision
log before admitting general traffic. Retain the old directory until this verification completes.

## Schema migration and rollback

Every persisted component has a schema/version header and refuses unsupported drift. A failed or
unrecovered migration therefore prevents startup/readiness rather than guessing. Before upgrading:

1. drain and stop;
2. take and verify a whole-root backup;
3. start the new Rust binary against a restored copy in staging;
4. run `cargo test --workspace`, `pnpm test:all`, and tenant-specific smoke turns;
5. promote the binary and data together.

Rollback means stop, restore the pre-upgrade backup to a new directory, and start the old binary.
Do not point an old binary at a directory already opened by a newer schema.

## Incident triage

Set `AELIO_DECISION_LOG=1` to emit redacted per-turn narratives. Logs include hashes, pins, tiers,
policy decisions, effect phases, and structural mismatch field names; raw tool arguments/results
and secrets are excluded. Preserve the data directory and logs before remediation.

- `503 not_ready`: verify storage permissions/schema and internal authentication configuration.
- `SigMismatch`: compare the SDK's declared stable output contract with the actual response shape.
- unknown effect outcome: do not retry blindly; inspect the intent/dispatch/result ledger and the
  tool's effect class/idempotency contract.
- SDK unavailable: the catalog remains pinned but calls fail closed until an authenticated host
  reconnects.

## Release evidence and known limitations

Required local gates:

```bash
cargo test --workspace --manifest-path aelio-os/Cargo.toml
cargo clippy --workspace --all-targets --manifest-path aelio-os/Cargo.toml -- -D warnings
pnpm test:all
node scripts/check-production-authority.mjs
```

The converter mutation harness currently catches 100% of its deliberately wrong converter set;
the executable assertion is `aelio-convert/tests/convert_gate.rs`. This is a finite mutation set,
not a claim of universal semantic correctness.

Residual limitations that must remain visible in a release:

- live-provider tests require operator-supplied credentials/network and are not counted by the
  deterministic local suite;
- the generic sandbox executes a closed, non-Turing-complete DSL in-process. Production HTTP host
  calls carry hard deadlines, but trusted in-process test closures cannot be forcibly interrupted;
- Forge v0 has a deliberately small admitted vendor catalog; expanding it requires new gated
  targets and evidence, not free-form model code;
- the complete long-duration load/soak and every master-plan crash-window scenario belong in
  release CI on the target storage/orchestrator. Passing local tests is not evidence for those
  environment-dependent guarantees.

