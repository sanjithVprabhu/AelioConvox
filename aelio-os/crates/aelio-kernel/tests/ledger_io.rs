use aelio_kernel::{
    compile,
    driver::{Instance, TurnOutcome},
    ledger::Ledger,
    registry::Registry,
    replay,
};
use aelio_sol::SolValue;

#[test]
fn persisted_ledger_round_trips_and_drives_real_replay() {
    let program =
        compile(r#"{"nid":"root","op":"Const","v":{"status":"persisted-replay"}}"#).unwrap();
    let initial = SolValue::map::<_, &str>([]);
    let mut registry = Registry::default();
    let mut instance = Instance::new(program.clone(), &mut registry);
    let live_hash = match instance.start(initial.clone()).unwrap() {
        TurnOutcome::Completed { bag_hash, .. } => bag_hash,
        TurnOutcome::Parked(_) => panic!("pure plan cannot park"),
    };

    let encoded = instance.ledger().to_json_pretty().unwrap();
    let decoded = Ledger::from_json_str(&encoded).unwrap();
    assert_eq!(replay(&program, &decoded, initial).unwrap(), live_hash);
}

#[test]
fn payload_tampering_is_detected_even_when_prev_is_unchanged() {
    let program = compile(r#"{"nid":"root","op":"Identity"}"#).unwrap();
    let mut registry = Registry::default();
    let mut instance = Instance::new(program, &mut registry);
    instance
        .start(SolValue::map::<_, &str>([]))
        .expect("live run");
    let encoded = instance.ledger().to_json_pretty().unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    json[0]["payload"]["trigger"] = serde_json::json!("tampered");
    let tampered = serde_json::to_string(&json).unwrap();
    assert!(Ledger::from_json_str(&tampered).is_err());
}
