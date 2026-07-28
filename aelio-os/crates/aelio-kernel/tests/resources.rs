use aelio_kernel::{
    compile,
    driver::{Instance, TurnOutcome},
    error::ReasonCode,
    registry::{Boundedness, Declaration, EffectClass, Invocation, Origin, Registry, TargetClass},
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
fn token_budget_uses_model_reported_usage_and_replays_the_trip() {
    let program = compile(
        r#"{"nid":"budget","op":"Budget","tokens":5,"body":{
          "nid":"model","op":"Call","id":"model.answer@1",
          "args":{"prompt_hash":{"lit":"hash-1"}},"into":"answer"
        }}"#,
    )
    .unwrap();
    let mut registry = Registry::default();
    registry
        .register_model_declared(
            Declaration {
                id: "model.answer@1".into(),
                class: TargetClass::Model,
                input_imprint: "model.in@1".into(),
                output_imprint: "model.out@1".into(),
                boundedness: Boundedness::DeadlineCompliant { max_ms: 1_000 },
                effect_class: EffectClass::Read,
                policy_tags: vec![],
                tenant: "default".into(),
                origin: Origin::Tenant,
            },
            |_| {
                Ok(Invocation {
                    output: SolValue::str("answer"),
                    usage_tokens: 6,
                })
            },
        )
        .unwrap();
    let mut instance = Instance::new(program, &mut registry);
    let error = match instance.start(SolValue::Map(Default::default())) {
        Err(error) => error,
        Ok(_) => panic!("reported model usage must trip Budget.tokens"),
    };
    assert_eq!(error.code, ReasonCode::BudgetTokens);
    assert!(instance.ledger().entries().iter().any(|entry| {
        entry.kind == "budget_trip"
            && entry.payload.as_map().and_then(|map| map.get("meter"))
                == Some(&SolValue::str("tokens"))
    }));
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

#[test]
fn once_default_key_tracks_read_projection_and_replays_recorded_writes() {
    let program = compile(
        r#"{"nid":"once","op":"Once","body":{
          "nid":"effect","op":"Call","id":"effect@1",
          "args":{"input":{"pull":"input"}},"into":"output"
        }}"#,
    )
    .unwrap();
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let mut registry = Registry::default();
    registry.register("effect@1", EffectClass::External, move |args| {
        observed.set(observed.get() + 1);
        Ok(args
            .as_map()
            .and_then(|map| map.get("input"))
            .cloned()
            .unwrap())
    });
    let mut instance = Instance::new(program, &mut registry);

    for input in [7, 7, 8] {
        let bag = match instance
            .start(SolValue::map([("input", SolValue::Int(input))]))
            .unwrap()
        {
            TurnOutcome::Completed { bag, .. } => bag,
            TurnOutcome::Parked(_) => panic!("Once body does not park"),
        };
        assert_eq!(
            bag.as_map().and_then(|map| map.get("output")),
            Some(&SolValue::Int(input))
        );
    }
    assert_eq!(
        calls.get(),
        2,
        "same read projection dedups; corrected input re-executes"
    );
}

#[test]
fn guard_each_checks_after_each_op_and_runs_the_declared_handler() {
    let program = compile(
        r#"{"nid":"guard","op":"Guard","check":"each",
          "invariant":{"pull":"allowed"},
          "body":{"nid":"steps","op":"Seq","steps":[
            {"nid":"flip","op":"Call","id":"flip@1","args":{},"into":"allowed"},
            {"nid":"must_not_run","op":"Call","id":"late@1","args":{},"into":"late"}
          ]},
          "on_violation":{"nid":"handled","op":"Call","id":"handler@1","args":{},"into":"handled"}
        }"#,
    )
    .unwrap();
    let late_calls = Rc::new(Cell::new(0));
    let observed = late_calls.clone();
    let mut registry = Registry::default();
    registry.register("flip@1", EffectClass::Pure, |_| Ok(SolValue::Bool(false)));
    registry.register("late@1", EffectClass::Pure, move |_| {
        observed.set(observed.get() + 1);
        Ok(SolValue::Bool(true))
    });
    registry.register("handler@1", EffectClass::Pure, |_| Ok(SolValue::Bool(true)));
    let mut instance = Instance::new(program, &mut registry);
    let bag = match instance
        .start(SolValue::map([("allowed", SolValue::Bool(true))]))
        .unwrap()
    {
        TurnOutcome::Completed { bag, .. } => bag,
        TurnOutcome::Parked(_) => panic!("guard handler does not park"),
    };
    assert_eq!(late_calls.get(), 0);
    assert_eq!(
        bag.as_map().and_then(|map| map.get("handled")),
        Some(&SolValue::Bool(true))
    );
    assert!(instance
        .ledger()
        .entries()
        .iter()
        .any(|entry| entry.kind == "guard_check"));
}
