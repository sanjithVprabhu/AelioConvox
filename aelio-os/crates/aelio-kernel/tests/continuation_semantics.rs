use aelio_kernel::{
    compile,
    driver::{Instance, TurnOutcome},
    registry::{EffectClass, Registry},
};
use aelio_sol::SolValue;

fn int_at(value: &SolValue, key: &str) -> Option<i64> {
    match value.as_map()?.get(key)? {
        SolValue::Int(value) => Some(*value),
        _ => None,
    }
}

#[test]
fn let_shadow_state_survives_park_and_is_unwound_after_resume() {
    let program = compile(
        r#"{
          "nid":"let","op":"Let",
          "bindings":[{"key":"temporary","value":{"lit":41}}],
          "body":{"nid":"body","op":"Seq","steps":[
            {"nid":"wait","op":"Park","until":{"kind":"event"}},
            {"nid":"copy","op":"Call","id":"copy@1",
             "args":{"value":{"pull":"temporary"}},"into":"result"}
          ]}
        }"#,
    )
    .unwrap();
    let mut registry = Registry::default();
    registry.register("copy@1", EffectClass::Pure, |args| {
        Ok(args
            .as_map()
            .and_then(|map| map.get("value"))
            .cloned()
            .unwrap_or(SolValue::Null))
    });
    let mut instance = Instance::new(program, &mut registry);

    let parked = match instance
        .start(SolValue::map([("stable", SolValue::Bool(true))]))
        .unwrap()
    {
        TurnOutcome::Parked(parked) => parked,
        TurnOutcome::Completed { .. } => panic!("Let body should park"),
    };
    assert_eq!(int_at(&parked.bag, "temporary"), Some(41));

    let completed = match instance.resume(parked, SolValue::Null).unwrap() {
        TurnOutcome::Completed { bag, .. } => bag,
        TurnOutcome::Parked(_) => panic!("Let body should complete after wake"),
    };
    assert_eq!(int_at(&completed, "result"), Some(41));
    assert!(
        completed
            .as_map()
            .is_some_and(|map| !map.contains_key("temporary")),
        "an originally absent lexical binding must be removed on scope exit"
    );
}

#[test]
fn loop_counter_and_mid_iteration_position_survive_multiple_parks() {
    let program = compile(
        r#"{
          "nid":"loop","op":"Loop",
          "while":{"fn":"lt","args":[{"pull":"count"},{"lit":2}]},
          "max_iter":3,
          "body":{"nid":"body","op":"Seq","steps":[
            {"nid":"wait","op":"Park","until":{"kind":"event"}},
            {"nid":"inc","op":"Call","id":"inc@1",
             "args":{"value":{"pull":"count"}},"into":"count"}
          ]}
        }"#,
    )
    .unwrap();
    let mut registry = Registry::default();
    registry.register("inc@1", EffectClass::Pure, |args| {
        let current = args
            .as_map()
            .and_then(|map| map.get("value"))
            .and_then(|value| match value {
                SolValue::Int(value) => Some(*value),
                _ => None,
            })
            .ok_or_else(|| {
                aelio_kernel::error::ErrV1::new(
                    aelio_kernel::error::ReasonCode::Type,
                    "inc",
                    "expected int",
                )
            })?;
        Ok(SolValue::Int(current + 1))
    });
    let mut instance = Instance::new(program, &mut registry);

    let first = match instance
        .start(SolValue::map([("count", SolValue::Int(0))]))
        .unwrap()
    {
        TurnOutcome::Parked(parked) => parked,
        TurnOutcome::Completed { .. } => panic!("iteration zero should park"),
    };
    let second = match instance.resume(first, SolValue::Null).unwrap() {
        TurnOutcome::Parked(parked) => parked,
        TurnOutcome::Completed { .. } => panic!("iteration one should park"),
    };
    assert_eq!(int_at(&second.bag, "count"), Some(1));

    let final_bag = match instance.resume(second, SolValue::Null).unwrap() {
        TurnOutcome::Completed { bag, .. } => bag,
        TurnOutcome::Parked(_) => panic!("loop should stop once count reaches two"),
    };
    assert_eq!(int_at(&final_bag, "count"), Some(2));
}

#[test]
fn map_retains_completed_elements_and_commits_atomically_after_park() {
    let program = compile(
        r#"{
          "nid":"map","op":"Map","over":"items","into":"mapped","max_items":3,
          "body":{"nid":"body","op":"Seq","steps":[
            {"nid":"wait","op":"Park","until":{"kind":"event"}},
            {"nid":"inc","op":"Call","id":"inc-element@1",
             "args":{"value":{"pull":"value"}},"into":"value"}
          ]}
        }"#,
    )
    .unwrap();
    let mut registry = Registry::default();
    registry.register("inc-element@1", EffectClass::Pure, |args| {
        let current = args
            .as_map()
            .and_then(|map| map.get("value"))
            .and_then(|value| match value {
                SolValue::Int(value) => Some(*value),
                _ => None,
            })
            .unwrap();
        Ok(SolValue::Int(current + 1))
    });
    let mut instance = Instance::new(program, &mut registry);
    let initial = SolValue::map([(
        "items",
        SolValue::list([
            SolValue::map([("value", SolValue::Int(10))]),
            SolValue::map([("value", SolValue::Int(20))]),
        ]),
    )]);

    let first = match instance.start(initial).unwrap() {
        TurnOutcome::Parked(parked) => parked,
        TurnOutcome::Completed { .. } => panic!("first element should park"),
    };
    assert!(
        first
            .bag
            .as_map()
            .is_some_and(|map| !map.contains_key("mapped")),
        "the parent landing path must not be partially committed"
    );

    let second = match instance.resume(first, SolValue::Null).unwrap() {
        TurnOutcome::Parked(parked) => parked,
        TurnOutcome::Completed { .. } => panic!("second element should park"),
    };
    assert_eq!(int_at(&second.bag, "value"), Some(20));

    let final_bag = match instance.resume(second, SolValue::Null).unwrap() {
        TurnOutcome::Completed { bag, .. } => bag,
        TurnOutcome::Parked(_) => panic!("Map should complete after the second wake"),
    };
    let mapped = final_bag
        .as_map()
        .and_then(|map| map.get("mapped"))
        .and_then(SolValue::as_list)
        .unwrap();
    assert_eq!(int_at(&mapped[0], "value"), Some(11));
    assert_eq!(int_at(&mapped[1], "value"), Some(21));
}

#[test]
fn try_handler_may_park_and_finally_waits_for_handler_completion() {
    let program = compile(
        r#"{
          "nid":"try","op":"Try","err_into":"caught",
          "body":{"nid":"fail","op":"Call","id":"fail@1","args":{},"into":"never"},
          "catch":{"Tool":{"nid":"handler","op":"Seq","steps":[
            {"nid":"wait","op":"Park","until":{"kind":"event"}},
            {"nid":"recover","op":"Call","id":"recover@1","args":{},"into":"result"}
          ]}},
          "finally":{"nid":"cleanup","op":"Call","id":"cleanup@1","args":{},"into":"cleanup"}
        }"#,
    )
    .unwrap();
    let mut registry = Registry::default();
    registry.register("fail@1", EffectClass::Read, |_| {
        Err(aelio_kernel::error::ErrV1::new(
            aelio_kernel::error::ReasonCode::ToolTransient,
            "fail",
            "expected",
        ))
    });
    registry.register("recover@1", EffectClass::Pure, |_| {
        Ok(SolValue::str("recovered"))
    });
    registry.register("cleanup@1", EffectClass::Pure, |_| Ok(SolValue::Bool(true)));
    let mut instance = Instance::new(program, &mut registry);

    let parked = match instance.start(SolValue::map::<_, &str>([])).unwrap() {
        TurnOutcome::Parked(parked) => parked,
        TurnOutcome::Completed { .. } => panic!("handler should park"),
    };
    assert!(
        parked
            .bag
            .as_map()
            .is_some_and(|map| !map.contains_key("cleanup")),
        "finally must not run when a Try body or handler parks"
    );

    let bag = match instance.resume(parked, SolValue::Null).unwrap() {
        TurnOutcome::Completed { bag, .. } => bag,
        TurnOutcome::Parked(_) => panic!("handler should finish after wake"),
    };
    assert_eq!(
        bag.as_map().and_then(|map| map.get("result")),
        Some(&SolValue::str("recovered"))
    );
    assert_eq!(
        bag.as_map().and_then(|map| map.get("cleanup")),
        Some(&SolValue::Bool(true))
    );
}
