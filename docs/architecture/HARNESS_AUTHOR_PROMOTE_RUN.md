# Author → Test → Promote → Run a Harness

This is the production path for adding programs to the Aelio Conductor/Harness OS.

## Concepts

| Term | Meaning |
|------|---------|
| **Harness** | Versioned Sol program with typed contract |
| **Draft** | Untrusted candidate (`harness_drafts_v1`) |
| **Sandbox** | Compile + plan + execute vectors |
| **Promote** | **Admin-only** install into live `sol_harness_contracts` |
| **Conductor** | Root shell that selects/spawns harnesses |

## 1. Author

Write a `HarnessContractV1` JSON (see `aelio-os/library/harnesses/*/1.0.0/contract.json`):

```json
{
  "id": "tool.run_once",
  "version": "1.0.0",
  "origin": "tenant",
  "description": "…",
  "input_imprint": "aelio.unit@1",
  "output_imprint": "aelio.tool.result@1",
  "errors": [],
  "effect": "external",
  "determinism": "ledgered_nondeterministic",
  "suspendability": "never",
  "children_allow": [],
  "budgets": { "steps": 20, "calls": 2, "wall_ms": 2000 },
  "program": { "nid": "root", "op": "Once", "body": { "…" } },
  "tags": ["tools"],
  "required_capabilities": ["tool.act_stub"]
}
```

`program` must be **canonical Sol** (17 control ops + registered `Call`s). Sugar is allowed only if it lowers to Sol before admission.

## 2. Draft

Rust API (`aelio_kernel::promote`):

```rust
let contract = /* HarnessContractV1 */;
let draft = draft_from_contract(contract)?;
save_draft(&mut store, tenant, draft)?;
```

## 3. Sandbox

```rust
let draft = sandbox_draft(
    &mut store,
    tenant,
    "tool.run_once",
    "1.0.0",
    &[(SolValue::Null, "sent")], // bag → expected field
)?;
assert_eq!(draft.status, DraftStatus::SandboxPassed);
```

Sandbox **must** pass before promote. Live chat never promotes.

## 4. Admin promote

```rust
let report = admin_promote(
    &mut store,
    tenant,
    "tool.run_once",
    "1.0.0",
    "admin@example.com", // non-empty principal required
)?;
```

Outcome: `Promoted` | `AlreadyPresent` | `Rejected`.

Audit row written to `harness_promote_audit_v1`.

## 5. Install vendor library (boot)

On-disk layout: `aelio-os/library/`

```bash
# Validate all Sol programs compile
aelio library validate --path aelio-os/library

# Install into durable store
aelio library install --tenant demo --path aelio-os/library --store /var/lib/aelio/os-store

# List manifest entries
aelio library list --path aelio-os/library
```

Rust:

```rust
let root = resolve_library_root(None).unwrap();
let report = install_from_manifest(&mut store, tenant, &root)?;
```

Idempotent by content hash. Existing ids are not overwritten.

**Catch-all:** unmatched free-form text runs installed `full_reply` Sol (not cold ProposePath) when the library is present.

## 6. Run

- **Deterministic greets** (`hi`, `thanks`, `bye`, …): `/v1/turns` cutover → `conductor.root` Sol (0 LLM).
- **Other turns**: agent spine + `Conductor.Shadow` comparison.
- **Events**: `POST /v2/events` admits `NormalizedEventV1` and can run Conductor.
- **Compose**: `workflow.average` spawn sum/count → join → divide; tree replay verifies bag hashes.

## 7. Process tree ops

| Op | Module |
|----|--------|
| spawn / join / cancel | `ProcessTree` |
| park / resume waits | `WaitPredicate` |
| budgets | `InstanceBudgetV1` |
| durable hydrate | `ProcessRepository` |
| cancel-safe effects | `Instance::mark_cancelled` |

## Safety rules

1. No self-promote from model judgment alone.
2. Effectful Calls: intent ledgered; cancel blocks post-cancel dispatch.
3. Once crash mid-intent → `UnknownOutcome` (fail closed).
4. Tenant cannot overwrite vendor pins in place (new version only).

## Env

| Variable | Meaning |
|----------|---------|
| `AELIO_OS_STORE_PATH` | Durable EmbeddedStore for OS tables (events, shadow, drafts) |

## Checklist

See `Blueprint/CONDUCTOR_HARNESS_E2E_CHECKLIST.md` for phase status.
