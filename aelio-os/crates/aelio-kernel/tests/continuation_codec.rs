use aelio_kernel::continuation::{
    mint_event_key, ContinuationIdentity, ContinuationPins, KERNEL_VERSION, SOL_VERSION,
};
use aelio_kernel::{compile, Instance, InstanceConfig, Parked, Registry, TurnOutcome};
use aelio_sol::SolValue;
use aelio_store::{EmbeddedStore, MemoryStore};

fn pins() -> ContinuationPins {
    ContinuationPins {
        kernel_version: KERNEL_VERSION.into(),
        sol_version: SOL_VERSION.into(),
        flow_id: "codec_flow".into(),
        flow_rev: "7".into(),
    }
}

fn identity() -> ContinuationIdentity {
    ContinuationIdentity {
        tenant: "tenant-a".into(),
        flow_instance_id: "instance-1".into(),
    }
}

#[test]
fn parked_flow_round_trips_and_resumes_without_reexecuting_prefix() {
    let program = compile(
        r#"{
          "nid":"root","op":"Seq","steps":[
            {"nid":"prefix","op":"Const","v":{"prefix":"kept"}},
            {"nid":"park","op":"Park","until":{"kind":"ttl","ms":5000},"into":"wake"},
            {"nid":"done","op":"Let","bindings":[
              {"key":"finished","value":{"lit":true}}
            ],"body":{"nid":"identity","op":"Identity"}}
          ]
        }"#,
    )
    .unwrap();
    let mut registry = Registry::default();
    let mut instance = Instance::new(program, &mut registry);
    let parked = match instance
        .start(SolValue::map([("ignored", SolValue::Bool(true))]))
        .unwrap()
    {
        TurnOutcome::Parked(parked) => parked,
        TurnOutcome::Completed { .. } => panic!("flow must park"),
    };
    let envelope = parked.to_continuation(&pins(), &identity()).unwrap();
    let decoded = Parked::from_continuation(&envelope, KERNEL_VERSION).unwrap();
    assert_eq!(decoded.pins, pins());
    assert_eq!(decoded.identity, identity());
    assert_eq!(decoded.parked.frame_nids.front().unwrap(), "root");

    let completed = instance
        .resume(decoded.parked, SolValue::str("payload"))
        .unwrap();
    let TurnOutcome::Completed { bag, .. } = completed else {
        panic!("flow must complete after wake");
    };
    assert_eq!(
        bag.as_map().unwrap().get("prefix"),
        Some(&SolValue::str("kept"))
    );
    assert_eq!(
        bag.as_map().unwrap().get("wake"),
        Some(&SolValue::str("payload"))
    );
    // Let is lexical: its temporary key is gone after completion.
    assert!(!bag.as_map().unwrap().contains_key("finished"));
}

#[test]
fn continuation_refuses_tampering_and_kernel_drift() {
    let program =
        compile(r#"{"nid":"park","op":"Park","until":{"kind":"event"},"into":"wake"}"#).unwrap();
    let mut registry = Registry::default();
    let mut instance = Instance::new(program, &mut registry);
    let parked = match instance
        .start(SolValue::map([("v", SolValue::Int(1))]))
        .unwrap()
    {
        TurnOutcome::Parked(parked) => parked,
        TurnOutcome::Completed { .. } => panic!("flow must park"),
    };
    let mut parked = parked;
    parked.event_key = Some("opaque-token".into());
    let mut envelope = parked.to_continuation(&pins(), &identity()).unwrap();
    let SolValue::Map(map) = &mut envelope else {
        panic!("continuation must be a map");
    };
    map.insert("cont_format".into(), SolValue::Int(2));
    assert!(Parked::from_continuation(&envelope, KERNEL_VERSION)
        .unwrap_err()
        .contains("hash mismatch"));

    let envelope = parked.to_continuation(&pins(), &identity()).unwrap();
    assert!(Parked::from_continuation(&envelope, "future-kernel")
        .unwrap_err()
        .contains("kernel version"));
}

#[test]
fn event_keys_are_secret_bound_and_context_separated() {
    let key_a = mint_event_key(&[7; 32], "t", "i", "p");
    assert_eq!(key_a, mint_event_key(&[7; 32], "t", "i", "p"));
    assert_ne!(key_a, mint_event_key(&[8; 32], "t", "i", "p"));
    assert_ne!(key_a, mint_event_key(&[7; 32], "t", "i", "other"));
}

#[test]
fn durable_instance_hydrates_ledger_and_continuation_after_restart() {
    let program = compile(
        r#"{"nid":"root","op":"Seq","steps":[
          {"nid":"wait","op":"Park","until":{"kind":"event"},"into":"wake"},
          {"nid":"finish","op":"Identity"}
        ]}"#,
    )
    .unwrap();
    let store = MemoryStore::new();
    let config = InstanceConfig {
        tenant: "tenant-a".into(),
        instance_id: "restart-1".into(),
        flow_id: "restart-flow".into(),
        flow_rev: "3".into(),
        event_key_secret: [9; 32],
    };

    {
        let mut registry = Registry::default();
        let mut instance = Instance::with_store(
            program.clone(),
            &mut registry,
            Box::new(store.clone()),
            config.clone(),
        )
        .unwrap();
        let TurnOutcome::Parked(parked) =
            instance.start(SolValue::Map(Default::default())).unwrap()
        else {
            panic!("flow must park");
        };
        assert!(parked.event_key.is_some());
    }

    let mut registry = Registry::default();
    let mut recovered =
        Instance::with_store(program, &mut registry, Box::new(store), config).unwrap();
    let TurnOutcome::Completed { bag, .. } = recovered
        .resume_stored(SolValue::str("after-restart"))
        .unwrap()
    else {
        panic!("recovered flow must complete");
    };
    assert_eq!(
        bag.as_map().unwrap().get("wake"),
        Some(&SolValue::str("after-restart"))
    );
    assert!(recovered.resume_stored(SolValue::Null).is_err());
    assert_eq!(recovered.ledger().entries().last().unwrap().turn_id, "t1");
}

#[test]
fn embedded_database_recovers_a_parked_instance_after_real_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let program = compile(
        r#"{"nid":"root","op":"Seq","steps":[
          {"nid":"wait","op":"Park","until":{"kind":"event"},"into":"wake"},
          {"nid":"finish","op":"Identity"}
        ]}"#,
    )
    .unwrap();
    let config = InstanceConfig {
        tenant: "tenant-db".into(),
        instance_id: "restart-db-1".into(),
        flow_id: "restart-flow".into(),
        flow_rev: "4".into(),
        event_key_secret: [11; 32],
    };

    {
        let mut registry = Registry::default();
        let store = EmbeddedStore::open(directory.path()).unwrap();
        let mut instance = Instance::with_store(
            program.clone(),
            &mut registry,
            Box::new(store),
            config.clone(),
        )
        .unwrap();
        assert!(matches!(
            instance.start(SolValue::Map(Default::default())).unwrap(),
            TurnOutcome::Parked(_)
        ));
    }

    let mut registry = Registry::default();
    let store = EmbeddedStore::open(directory.path()).unwrap();
    let mut recovered =
        Instance::with_store(program, &mut registry, Box::new(store), config).unwrap();
    let TurnOutcome::Completed { bag, .. } = recovered
        .resume_stored(SolValue::str("durable-wake"))
        .unwrap()
    else {
        panic!("reopened embedded database must resume the parked flow");
    };
    assert_eq!(
        bag.as_map().and_then(|map| map.get("wake")),
        Some(&SolValue::str("durable-wake"))
    );
    assert!(recovered.resume_stored(SolValue::Null).is_err());
}
