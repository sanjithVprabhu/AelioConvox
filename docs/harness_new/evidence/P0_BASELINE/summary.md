# P0 baseline verification

**Date:** 2026-08-09  
**Scope:** Read-only baseline of the existing Rust workspace before agent-loop production integration.

## Commands

### `cargo test --workspace`

Result: **FAILED** with exit code 101 after compiling the existing workspace.

Pre-existing warnings observed before the new crate ran:

- unused import `HARNESS_INVOKE_MAX_DEPTH` in `aelio-kernel/tests/exhaustive_harness_system.rs`;
- unused initial assignment to `dispatch_count` in `aelio-kernel/tests/crash_injection.rs`;
- unused `Result` in `aelio-agent/examples/aelio_12_simulations.rs`.

Pre-existing failing tests in `aelio-agent-api/tests/api.rs`:

1. `admitted_bound_procedure_executes_through_unified_runtime_and_replays`
   - expected tier `tier2`, received `tier3` at line 667.
2. `learning_control_plane_can_inspect_and_kill_a_promoted_procedure`
   - `Option::unwrap()` on `None` at line 2366.
3. `unmaterialized_public_flow_fails_closed_before_a_runtime_proxy_effect`
   - missing expected closed materialization decision at line 1525.

The baseline stopped after this test binary, so later workspace binaries were not executed by Cargo.

### `cargo fmt --all -- --check`

Result: **FAILED** because many existing, user-modified Rust files are not rustfmt-clean. No whole-workspace formatting rewrite was applied because those changes belong to the user.

## New isolated crate checks at P0

### `cargo test -p aelio-agent-loop`

Result at the initial checkpoint: **PASS** — 5 passed, 0 failed. The completed crate now has 27 passing tests; see the P4 evidence.

### `cargo fmt -p aelio-agent-loop -- --check`

Result: **PASS**.

### `cargo clippy -p aelio-agent-loop --all-targets -- -D warnings`

Result: **PASS**.

## Initial gate disposition

P0 distinguished the three existing API failures from new-harness regressions and prevented them from being hidden by changed expectations. Subsequent implementation proceeded behind the explicit `agent_loop` authority branch. The complete non-API Rust workspace is now green, while the same three legacy API assertions and pre-existing whole-workspace warning/format debt remain documented in [`../../IMPLEMENTATION_AUDIT.md`](../../IMPLEMENTATION_AUDIT.md).
