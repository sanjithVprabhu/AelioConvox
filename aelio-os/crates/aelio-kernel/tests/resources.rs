use aelio_kernel::{
    compile,
    driver::{Instance, TurnOutcome},
    error::ReasonCode,
    registry::{EffectClass, Registry},
};
use aelio_sol::SolValue;
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn call_budget_spans_park_and_blocks_the_excess_call_before_dispatch() {
    let program = compile(
        r#"{"nid":"budget","op":"Budget","calls":1,"body":{
          "nid":"steps","op":"Seq","steps":[
            {"nid":"first","op":"Call","id":"count@1","args":{},"into":"first"},
            {"nid":"wait","op":"Park","until":{"kind":"event"}},
            {"nid":"second","op":"Call","id":"count@1","args":{},"into":"second"}
          ]
        }}"#,
    )
    .unwrap();
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let mut registry = Registry::default();
    registry.register("count@1", EffectClass::Read, move |_| {
        observed.set(observed.get() + 1);
        Ok(SolValue::Int(observed.get()))
    });
    let mut instance = Instance::new(program, &mut registry);
    let parked = match instance.start(SolValue::map::<_, &str>([])).unwrap() {
        TurnOutcome::Parked(parked) => parked,
        TurnOutcome::Completed { .. } => panic!("budget body should park"),
    };
    let error = match instance.resume(parked, SolValue::Null) {
        Err(error) => error,
        Ok(_) => panic!("second Call must exceed the persisted call budget"),
    };
    assert_eq!(error.code, ReasonCode::BudgetCalls);
    assert_eq!(calls.get(), 1, "excess Call must not dispatch");
    assert!(instance
        .ledger()
        .entries()
        .iter()
        .any(|entry| entry.kind == "budget_trip"));
    assert_eq!(
        instance
            .ledger()
            .entries()
            .last()
            .map(|entry| entry.kind.as_str()),
        Some("turn_end"),
        "errored turns must be terminally ledgered"
    );
}

#[test]
fn timeout_trips_are_ledgered() {
    let program = compile(
        r#"{"nid":"timeout","op":"Timeout","ms":1,"body":{
          "nid":"slow","op":"Call","id":"slow@1","args":{},"into":"out"
        }}"#,
    )
    .unwrap();
    let mut registry = Registry::default();
    registry.register("slow@1", EffectClass::Read, |_| {
        std::thread::sleep(std::time::Duration::from_millis(3));
        Ok(SolValue::Bool(true))
    });
    let mut instance = Instance::new(program, &mut registry);
    let error = match instance.start(SolValue::map::<_, &str>([])) {
        Err(error) => error,
        Ok(_) => panic!("slow call must time out"),
    };
    assert_eq!(error.code, ReasonCode::Timeout);
    assert!(instance
        .ledger()
        .entries()
        .iter()
        .any(|entry| entry.kind == "timeout_trip"));
}
