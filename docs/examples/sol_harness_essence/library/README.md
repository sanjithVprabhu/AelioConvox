# Sol harness library (saved contracts)

Contracts live in table `sol_harness_contracts`. Count = `sol_harness_library().len()` (33+ including nested stack).

Exhaustive ops list: [`../EXHAUSTIVE_OPS_AND_HARNESSES.md`](../EXHAUSTIVE_OPS_AND_HARNESSES.md)  
Nested A→B→C wait: [`../NESTED_HARNESS_STACK.md`](../NESTED_HARNESS_STACK.md)

## Index

See [`catalog.json`](./catalog.json).

## API

```rust
use aelio_kernel::{sol_harness_library, store_library, load_contract};
use aelio_store::MemoryStore;

let mut store = MemoryStore::new();
store_library(&mut store, "demo")?;
let n = sol_harness_library().len();
assert!(n >= 33);
```

```bash
cd aelio-os
cargo test -p aelio-kernel sol_harness
# refresh JSON mirrors:
cargo test -p aelio-kernel dump_library_json_files -- --ignored
```

Rust source of truth: `aelio-os/crates/aelio-kernel/src/sol_harness_lib.rs`
