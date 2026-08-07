#!/usr/bin/env bash
# D1/A4: compare deterministic hash probes across distinct executable processes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OS_ROOT="$ROOT/aelio-os"
PROBE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/aelio-restart-hash.XXXXXX")"

cleanup() {
  rm -rf "$PROBE_DIR"
}
trap cleanup EXIT

cat >"$PROBE_DIR/Cargo.toml" <<EOF
[package]
name = "aelio-restart-hash-probe"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
aelio-sol = { path = "$OS_ROOT/crates/aelio-sol" }
harness-core = { path = "$OS_ROOT/crates/harness-core" }
EOF

mkdir -p "$PROBE_DIR/src"
cat >"$PROBE_DIR/src/main.rs" <<'EOF'
use aelio_sol::{value_hash, SolValue};
use harness_core::agg::float_sum_reference_hash;

fn main() {
    let canonical_map = SolValue::map([
        ("b", SolValue::Int(2)),
        (
            "a",
            SolValue::list([SolValue::Float(0.1 + 0.2), SolValue::str("xy")]),
        ),
    ]);

    println!("canonical_map={}", value_hash(&canonical_map));
    println!("float_sum={}", float_sum_reference_hash());
}
EOF

run_probe() {
  (
    cd "$OS_ROOT"
    cargo test -p harness-core --test determinism --locked >&2
    CARGO_TARGET_DIR="$OS_ROOT/target" cargo run --quiet --manifest-path "$PROBE_DIR/Cargo.toml"
  )
}

first="$(run_probe)"
second="$(run_probe)"

if ! diff -u <(printf '%s\n' "$first") <(printf '%s\n' "$second"); then
  echo "RESTART HASH DRILL: FAIL (hash output changed across processes)" >&2
  exit 1
fi

printf '%s\n' "$first"
echo "RESTART HASH DRILL: PASS"
