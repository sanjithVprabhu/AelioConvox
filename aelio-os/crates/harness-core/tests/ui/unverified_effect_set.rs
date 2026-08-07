use harness_core::{authorize, EffectSet};

fn main() {
    let raw = EffectSet::from_effects(["tool.delete_user"]);
    let _ = authorize(&raw, "tool.delete_user");
}
