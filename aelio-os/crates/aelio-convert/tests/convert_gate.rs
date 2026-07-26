//! Conversion + gate conformance (§13–§16). Covers the App A e1/e2 converters verbatim, the closed
//! rule set semantics, non-computation, fabrication/reach tier escalation, the lifecycle machine,
//! and the mutation-harness catch rate (§16.6).

use aelio_convert::{
    apply_rules, gate, lifecycle, parse_rules, run_harness, Digest, Evidence, Reach, Rule, Status,
    Thresholds, Tier, Trigger,
};
use aelio_sol::SolValue;

fn json_rules(text: &str) -> Vec<Rule> {
    parse_rules(&serde_json::from_str(text).unwrap()).expect("rules parse")
}

// ── App A e1: auth_raw {active} → pred digest {loggedin} via rename ────────────────────────────
#[test]
fn app_a_e1_rename_active_to_loggedin() {
    let rules = json_rules(r#"[{"op":"rename","from":"active","to":"loggedin"}]"#);
    let input = SolValue::map([("active", SolValue::Bool(true))]);
    let out = apply_rules(&rules, &input).unwrap();
    assert_eq!(
        out.as_map().unwrap().get("loggedin"),
        Some(&SolValue::Bool(true))
    );
    assert!(out.as_map().unwrap().get("active").is_none(), "renamed away");

    // e1 reaches SendOtp (external) ⇒ reviewed tier — the founding-bug check (§16.2).
    let reach = Reach { reaches_write_or_external: true, ..Default::default() };
    assert_eq!(gate::classify_tier(&reach, &rules), Tier::Reviewed);
}

// ── App A e2: reply2 → verify digest via trim + cast str→int ───────────────────────────────────
#[test]
fn app_a_e2_trim_and_cast() {
    let rules = json_rules(r#"[{"op":"trim","path":"code"},{"op":"cast","path":"code","to":"int"}]"#);
    let input = SolValue::map([("code", SolValue::str("  434543  "))]);
    let out = apply_rules(&rules, &input).unwrap();
    assert_eq!(out.as_map().unwrap().get("code"), Some(&SolValue::Int(434543)));
}

#[test]
fn map_enum_never_invents_on_unmapped() {
    let rules = json_rules(r#"[{"op":"map_enum","path":"status","table":{"A":"active","I":"inactive"}}]"#);
    // Mapped value works.
    let ok = apply_rules(&rules, &SolValue::map([("status", SolValue::str("A"))])).unwrap();
    assert_eq!(ok.as_map().unwrap().get("status"), Some(&SolValue::str("active")));
    // Unmapped value ⇒ RuleFail; consumer never runs on unmapped data (§14, A14.2).
    let err = apply_rules(&rules, &SolValue::map([("status", SolValue::str("Z"))])).unwrap_err();
    assert_eq!(err.rule_index, 0);
}

#[test]
fn cast_matrix_forbids_semantic_casts() {
    // int→bool is FORBIDDEN (§5.2) — meaning-bearing, must go through map_enum.
    let rules = json_rules(r#"[{"op":"cast","path":"x","to":"bool"}]"#);
    let err = apply_rules(&rules, &SolValue::map([("x", SolValue::Int(1))])).unwrap_err();
    assert_eq!(err.rule_index, 0);
    // float→int without mode is plan-time-shaped rejection at apply.
    let no_mode = json_rules(r#"[{"op":"cast","path":"x","to":"int"}]"#);
    assert!(apply_rules(&no_mode, &SolValue::map([("x", SolValue::float(1.9).unwrap())])).is_err());
    // With mode it succeeds.
    let with_mode = json_rules(r#"[{"op":"cast","path":"x","to":"int","mode":"floor"}]"#);
    let out = apply_rules(&with_mode, &SolValue::map([("x", SolValue::float(1.9).unwrap())])).unwrap();
    assert_eq!(out.as_map().unwrap().get("x"), Some(&SolValue::Int(1)));
}

#[test]
fn fabricating_rules_escalate_to_reviewed_regardless_of_reach() {
    // A `const_set` on a pure conversational edge (no reach) still escalates (§14 fabrication).
    let rules = json_rules(r#"[{"op":"const_set","path":"role","v":"admin"}]"#);
    let no_reach = Reach::default();
    assert_eq!(gate::classify_tier(&no_reach, &rules), Tier::Reviewed);
    assert!(aelio_convert::any_fabricating(&rules));
    // A pure rename with no reach stays auto.
    let pure = json_rules(r#"[{"op":"rename","from":"a","to":"b"}]"#);
    assert_eq!(gate::classify_tier(&Reach::default(), &pure), Tier::Auto);
    // secret ⇒ locked.
    let secret_reach = Reach { is_secret: true, ..Default::default() };
    assert_eq!(gate::classify_tier(&secret_reach, &pure), Tier::Locked);
}

#[test]
fn rules_are_non_computational_by_construction() {
    // A14.4: the closed set has no op that references the rule list, no loop, no self-reference —
    // structurally, no rule variant can carry another rule or select rules. Enumerate the variants
    // to prove none is program-bearing.
    let sample = json_rules(
        r#"[{"op":"rename","from":"a","to":"b"},{"op":"drop","path":"c"},
            {"op":"keep","paths":["b"]},{"op":"default","path":"d","v":1},
            {"op":"cast","path":"b","to":"str"},{"op":"wrap","path":"b","key":"w"},
            {"op":"unwrap","path":"b"},{"op":"map_enum","path":"b","table":{"x":"y"}},
            {"op":"path_copy","from":"b","to":"e"},{"op":"const_set","path":"f","v":2},
            {"op":"trim","path":"b"}]"#,
    );
    for r in &sample {
        // Each rule's payload is data/paths only — never an Instruction or another Rule.
        let _program_free = matches!(
            r,
            Rule::Rename { .. } | Rule::Drop { .. } | Rule::Keep { .. } | Rule::Default { .. }
                | Rule::Cast { .. } | Rule::Wrap { .. } | Rule::Unwrap { .. } | Rule::MapEnum { .. }
                | Rule::PathCopy { .. } | Rule::ConstSet { .. } | Rule::Trim { .. }
        );
        assert!(_program_free);
    }
    // Unknown ops are rejected — the proposer's surface is exactly this set (containment, §15).
    assert!(parse_rules(&serde_json::json!([{"op":"exec","body":"..."}])).is_err());
}

#[test]
fn lifecycle_transitions_follow_app_k() {
    use lifecycle::transition;
    assert_eq!(transition(Status::Proposed, Trigger::StructuralPass).unwrap(), Status::Shadow);
    assert_eq!(transition(Status::Proposed, Trigger::StructuralFail).unwrap(), Status::Rejected);
    // Reviewed tier: approval gates first consumption (shadow→canary), not promotion.
    assert!(transition(Status::Shadow, Trigger::ShadowThresholdsMet { approved: false }).is_err());
    assert_eq!(transition(Status::Shadow, Trigger::ShadowThresholdsMet { approved: true }).unwrap(), Status::Canary);
    assert_eq!(transition(Status::Canary, Trigger::CanaryThresholdsMet).unwrap(), Status::Promoted);
    // Demotion is an event → shadow; kernel bump → canary-all.
    assert_eq!(transition(Status::Promoted, Trigger::AttributedFailure).unwrap(), Status::Shadow);
    assert_eq!(transition(Status::Promoted, Trigger::KernelBump).unwrap(), Status::Canary);
}

#[test]
fn thresholds_over_distinct_inputs() {
    let th = Thresholds::default();
    // 19 distinct inputs is below the bar even at 100% validation (§16.4: ≥20 distinct).
    let short = Evidence { shadow_distinct: 19, shadow_validation_rate: 1.0, ..Default::default() };
    assert!(!gate::shadow_to_canary(&short, &th));
    let ok = Evidence { shadow_distinct: 20, shadow_validation_rate: 0.96, ..Default::default() };
    assert!(gate::shadow_to_canary(&ok, &th));
}

// ── §16.6 mutation harness: deliberately-wrong converters, published catch rate ────────────────
#[test]
fn mutation_harness_catches_wrong_converters() {
    // Consumer wants {loggedin: bool}. Correct converter renames active→loggedin.
    let digest = Digest::new([("loggedin", "bool")]);
    let inputs: Vec<SolValue> = (0..25)
        .map(|i| SolValue::map([("active", SolValue::Bool(i % 2 == 0)), ("noise", SolValue::Int(i))]))
        .collect();

    // Mutants: each a plausible-but-wrong converter.
    let mutants = vec![
        json_rules(r#"[{"op":"drop","path":"active"}]"#),                     // drops the needed field
        json_rules(r#"[{"op":"rename","from":"active","to":"wrongkey"}]"#),   // renames to the wrong key
        json_rules(r#"[{"op":"rename","from":"nonexistent","to":"loggedin"}]"#), // renames from a missing key
        json_rules(r#"[{"op":"const_set","path":"loggedin","v":"true"}]"#),   // wrong type (str, not bool)
        json_rules(r#"[{"op":"map_enum","path":"active","table":{"true":"loggedin"}}]"#), // nonsense mapping
    ];

    let report = run_harness(&inputs, &digest, &mutants, &Thresholds::default());
    assert_eq!(report.total, 5);
    assert_eq!(report.escaped, 0, "no wrong converter should reach canary");
    assert_eq!(report.catch_rate(), 1.0, "§16.6 published catch rate = 100% on this set");

    // And the CORRECT converter passes shadow (would advance).
    let correct = json_rules(r#"[{"op":"rename","from":"active","to":"loggedin"}]"#);
    let passes = inputs.iter().filter(|i| gate::shadow_validate(&correct, i, &digest)).count();
    assert_eq!(passes, inputs.len(), "correct converter validates on every input");
}

// ── §13 conversion-edge lifecycle + on_parse_fail + rejected short-circuit ─────────────────────
#[test]
fn edge_advances_through_lifecycle_and_honors_on_parse_fail() {
    use aelio_convert::{ConversionEdge, ConvertUseError, EdgeId, OnParseFail, Sensitivity, Status, Trigger};
    let rules = json_rules(r#"[{"op":"rename","from":"active","to":"loggedin"}]"#);
    let mut edge = ConversionEdge {
        id: EdgeId { tenant: "t".into(), flow_id: "login.v1".into(), producer_nid: "n_read".into(), consumer_nid: "n_branch".into() },
        conversion_id: "conv1".into(),
        version: 1,
        status: Status::Proposed,
        rules: rules.clone(),
        rules_hash: aelio_convert::rules_hash(&rules),
        from_signature: aelio_sol::structural_imprint(&SolValue::map([("active", SolValue::Bool(true))])),
        to_digest: aelio_convert::Digest::new([("loggedin", "bool")]),
        on_parse_fail: OnParseFail::Error,
        sensitivity: Sensitivity::Internal,
        evidence: aelio_convert::Evidence::default(),
    };
    // proposed → shadow → (reviewed approval) canary → promoted.
    edge.advance(Trigger::StructuralPass).unwrap();
    assert_eq!(edge.status, Status::Shadow);
    edge.advance(Trigger::ShadowThresholdsMet { approved: true }).unwrap();
    assert_eq!(edge.status, Status::Canary);
    edge.advance(Trigger::CanaryThresholdsMet).unwrap();
    assert_eq!(edge.status, Status::Promoted);

    // Warm use: the rename applies.
    let out = edge.apply_at_use(&SolValue::map([("active", SolValue::Bool(true))])).unwrap();
    assert_eq!(out.as_map().unwrap().get("loggedin"), Some(&SolValue::Bool(true)));

    // A RuleFail with on_parse_fail=Error propagates Convert.RuleFail (input lacks `active`).
    let err = edge.apply_at_use(&SolValue::map([("other", SolValue::Bool(true))])).unwrap_err();
    assert!(matches!(err, ConvertUseError::RuleFail { .. }));

    // With on_parse_fail=Default, the same failing input yields the default instead.
    edge.on_parse_fail = OnParseFail::Default(SolValue::map([("loggedin", SolValue::Bool(false))]));
    let defaulted = edge.apply_at_use(&SolValue::map([("other", SolValue::Bool(true))])).unwrap();
    assert_eq!(defaulted.as_map().unwrap().get("loggedin"), Some(&SolValue::Bool(false)));
}

#[test]
fn rejected_rules_hash_short_circuits_repeat_proposals() {
    use aelio_convert::RejectedRegistry;
    let bad = json_rules(r#"[{"op":"rename","from":"x","to":"y"}]"#);
    let h = aelio_convert::rules_hash(&bad);
    let mut reg = RejectedRegistry::default();
    assert!(!reg.is_rejected(&h));
    reg.reject(h.clone(), "structural: consumer digest unmet");
    // Same rules re-proposed → short-circuit to backoff (§13.1).
    assert!(reg.is_rejected(&aelio_convert::rules_hash(&bad)));
    // A *different* rules_hash starts fresh.
    let other = json_rules(r#"[{"op":"rename","from":"x","to":"z"}]"#);
    assert!(!reg.is_rejected(&aelio_convert::rules_hash(&other)));
}
