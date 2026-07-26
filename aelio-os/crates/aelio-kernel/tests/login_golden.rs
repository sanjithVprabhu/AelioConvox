//! Golden test #1 — the Appendix A login flow, end-to-end, with **replay bit-identity** (G2).
//! This is P0's definition of done (§30). The flow is a faithful reduction of App A: sense hydrate →
//! state read → Branch(unauthenticated) → [ask_phone → Park → validate → Guard → Once(Budget(send))
//! → ask_otp → Park → verify → state_write → express]. Two parks, two resumes, one completion; then
//! the whole per-instance ledger is replayed as a pure function and the final bag_hash must match.

use aelio_kernel::{compile, replay, EffectClass, Instance, Registry, TurnOutcome};
use aelio_sol::SolValue;

fn login_program() -> &'static str {
    r#"
    {"nid":"root","op":"Seq","steps":[
      {"nid":"n_hyd","op":"Call","id":"io.sense_hydrate@1","args":{},"into":"hydration_meta"},
      {"nid":"n_read","op":"Call","id":"io.state_read@1","args":{"key":{"lit":"deployer_auth"}},"into":"auth_raw"},
      {"nid":"n_branch","op":"Branch",
        "pred":{"fn":"eq","args":[{"pull":"auth_raw.loggedin"},{"lit":true}]},
        "then":{"nid":"n_authed","op":"Call","id":"flow.authed_menu@1","args":{},"into":"r"},
        "else":{"nid":"n_login","op":"Seq","steps":[
          {"nid":"n_askp","op":"Call","id":"model.ask_phone@1","args":{"lang":{"lit":"en"}},"into":"ask1"},
          {"nid":"n_park1","op":"Park","until":{"kind":"event"},"into":"reply1"},
          {"nid":"n_valp","op":"Call","id":"compute.validate_phone@1","args":{"v":{"pull":"reply1.text"}},"into":"phone"},
          {"nid":"n_guard","op":"Guard","check":"entry",
            "invariant":{"fn":"exists","args":[{"pull":"phone.e164"}]},
            "body":{"nid":"n_once","op":"Once",
              "body":{"nid":"n_budget","op":"Budget","calls":3,"ms":20000,
                "body":{"nid":"n_send","op":"Call","id":"tool.send_otp@1","args":{"to":{"pull":"phone.e164"}},"into":"otp_send"}}}},
          {"nid":"n_asko","op":"Call","id":"model.ask_otp@1","args":{},"into":"ask2"},
          {"nid":"n_park2","op":"Park","until":{"kind":"event"},"into":"reply2"},
          {"nid":"n_verify","op":"Call","id":"tool.verify_otp@1","args":{"code":{"pull":"reply2.code"}},"into":"verify"},
          {"nid":"n_write","op":"Call","id":"io.state_write@1","args":{"key":{"lit":"deployer_auth"},"v":{"lit":{"loggedin":true}}},"into":"sw"},
          {"nid":"n_ok","op":"Call","id":"model.express_success@1","args":{},"into":"out"}
        ]}
      }
    ]}
    "#
}

fn login_registry() -> Registry {
    let mut r = Registry::default();
    r.register("io.sense_hydrate@1", EffectClass::Read, |_| Ok(SolValue::map::<_, &str>([])));
    // Unauthenticated → Branch takes the else (login) path.
    r.register("io.state_read@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("loggedin", SolValue::Bool(false))]))
    });
    r.register("model.ask_phone@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("text", SolValue::str("What's your number?"))]))
    });
    r.register("compute.validate_phone@1", EffectClass::Pure, |_args| {
        // A real validator is a registered target (§5.4); the stub normalizes to E164.
        Ok(SolValue::map([("e164", SolValue::str("+919876543210"))]))
    });
    r.register("tool.send_otp@1", EffectClass::External, |_| {
        Ok(SolValue::map([("sent", SolValue::Bool(true))]))
    });
    r.register("model.ask_otp@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("text", SolValue::str("Enter the 6-digit code"))]))
    });
    r.register("tool.verify_otp@1", EffectClass::External, |args| {
        let ok = args
            .as_map()
            .and_then(|m| m.get("code"))
            .and_then(|c| if let SolValue::Str(s) = c { Some(s.as_str()) } else { None })
            == Some("434543");
        Ok(SolValue::map([("ok", SolValue::Bool(ok))]))
    });
    r.register("io.state_write@1", EffectClass::Write, |_| Ok(SolValue::map::<_, &str>([])));
    r.register("model.express_success@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("text", SolValue::str("You're in."))]))
    });
    r.register("flow.authed_menu@1", EffectClass::Read, |_| Ok(SolValue::map::<_, &str>([])));
    r
}

#[test]
fn login_flow_end_to_end_with_replay_bit_identity() {
    let program = compile(login_program()).expect("login flow compiles + passes the Planner");
    let mut registry = login_registry();
    let mut instance = Instance::new(program.clone(), &mut registry);

    // Turn 1: runs to the first park (awaiting the phone number).
    let parked1 = match instance.start(SolValue::map::<_, &str>([])).unwrap() {
        TurnOutcome::Parked(p) => {
            assert_eq!(p.park_nid, "n_park1", "first park is the phone ask");
            p
        }
        TurnOutcome::Completed { .. } => panic!("should have parked at n_park1"),
    };

    // Turn 2: user replies with a phone; runs send_otp, parks awaiting the OTP.
    let wake_phone = SolValue::map([("text", SolValue::str("98765 43210"))]);
    let parked2 = match instance.resume(parked1, wake_phone).unwrap() {
        TurnOutcome::Parked(p) => {
            assert_eq!(p.park_nid, "n_park2", "second park is the OTP ask");
            p
        }
        TurnOutcome::Completed { .. } => panic!("should have parked at n_park2"),
    };

    // Turn 3: user replies with the OTP; verifies, writes state, completes.
    let wake_otp = SolValue::map([("code", SolValue::str("434543"))]);
    let (bag, bag_hash) = match instance.resume(parked2, wake_otp).unwrap() {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
        TurnOutcome::Parked(_) => panic!("should have completed"),
    };

    // The flow reached the authenticated write + success express.
    let verify_ok = bag
        .as_map()
        .and_then(|m| m.get("verify"))
        .and_then(|v| v.as_map())
        .and_then(|m| m.get("ok"));
    assert_eq!(verify_ok, Some(&SolValue::Bool(true)), "OTP verified");
    assert!(bag.as_map().unwrap().contains_key("out"), "success expressed");

    // Ledger integrity: per-instance chain gapless + linked (App G, §28).
    instance.ledger().verify_chain().expect("ledger chain gapless + linked");

    // ── G2: replay the whole instance ledger as a pure function; bit-identity or hard refuse. ──
    let recomputed = replay(&program, instance.ledger(), SolValue::map::<_, &str>([]))
        .expect("replay must not diverge");
    assert_eq!(recomputed, bag_hash, "replay bag_hash bit-identical (§12.3, G2)");
}

#[test]
fn planner_rejects_writes_under_sense() {
    // §8.4: the reserved runtime-owned `sense` subtree is unwritable — cheap, load-bearing test.
    let program = r#"{"nid":"bad","op":"Call","id":"io.x@1","args":{},"into":"sense.env.now"}"#;
    let err = compile(program).expect_err("must reject a write under sense");
    assert_eq!(err.code, aelio_kernel::ReasonCode::Policy);
}

#[test]
fn planner_rejects_park_in_tee_side() {
    // §8.4 matrix: a suspending side effect contradicts fire-and-record.
    let program = r#"
    {"nid":"t","op":"Tee","side_root":"log",
      "body":{"nid":"b","op":"Identity"},
      "side":{"nid":"p","op":"Park","until":{"kind":"event"},"into":"log.x"}}"#;
    assert!(compile(program).is_err(), "Park inside Tee.side is plan-time rejected");
}
