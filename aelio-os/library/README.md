# Aelio standard library (on-disk)

Authority: `Blueprint/conductor-harness-os-production-plan.md` §7.2.

This directory is the **filesystem source** for vendor harnesses and Call targets.
Artifacts are versioned, content-hashed at install, and never mutated in place.

## Layout

```
library/
  manifest.yaml|json
  harnesses/<family>/<id>/<version>/
    contract.yaml|json
    program.sol.json
    examples/
  targets/<family>/
  imprints/
  prompts/
```

## Bootstrap slice (Phase 1)

| ID | Status |
|----|--------|
| `workflow.average@1.0.0` | Sol program + contract + vectors |
| `math.sum@1` | pure Call target |
| `math.divide@1` | pure Call target |
| `collection.count@1` | pure Call target |

## Execution

Programs lower to / already are canonical Sol JSON. The kernel compiles them via
`aelio_kernel::compile` and runs them with a registry that includes
`register_p0_pure_stdlib`.

Parallel `SpawnMany [sum, count]` is specified by the plan but **not** claimed here;
this slice is sequential pure composition with bag-hash replay.
