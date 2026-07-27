//! §16.5 — the gate is generic. Converters, pathways, and procedures all feed the *same*
//! `shadow_evidence` accumulator + `shadow_to_canary` threshold; only the agreement predicate
//! differs per class. This test drives all three through the identical machinery.

use aelio_convert::{gate, parse_rules, shadow_evidence, Digest, Thresholds};
use aelio_learn::{pathway_agreement, procedure_agreement, PathwayTurn, TraceStep};
use aelio_sol::SolValue;

fn advances(obs: &[(String, bool)]) -> bool {
    let ev = shadow_evidence(obs.iter().map(|(h, a)| (h.as_str(), *a)));
    gate::shadow_to_canary(&ev, &Thresholds::default())
}

#[test]
fn converter_pathway_procedure_share_one_gate() {
    // ── Converter: agreement = digest validation (§16.1.2). 20 distinct validating inputs. ──
    let rules =
        parse_rules(&serde_json::json!([{"op":"rename","from":"active","to":"loggedin"}])).unwrap();
    let digest = Digest::new([("loggedin", "bool")]);
    let conv_obs: Vec<(String, bool)> = (0..20)
        .map(|i| {
            let input = SolValue::map([("active", SolValue::Bool(i % 2 == 0))]);
            (
                format!("conv-{i}"),
                gate::shadow_validate(&rules, &input, &digest),
            )
        })
        .collect();
    assert!(
        advances(&conv_obs),
        "correct converter advances through the shared gate"
    );

    // ── Pathway: agreement = retrospective outcome match on incumbent-failed turns (§18). ──
    let pw_obs: Vec<(String, bool)> = (0..30)
        .filter_map(|i| {
            let turn = PathwayTurn {
                incumbent_failed: i % 3 != 0,
                new_would_succeed: true,
            };
            pathway_agreement(&turn).map(|agreed| (format!("pw-{i}"), agreed))
        })
        .collect();
    assert!(pw_obs.len() >= 20, "enough incumbent-failed turns to judge");
    assert!(
        advances(&pw_obs),
        "a challenger that fixes incumbent failures advances (same gate)"
    );

    // ── Procedure: agreement = shadow-compare predicted vs actual sequence (§19). ──
    let predicted = vec![
        TraceStep {
            call_id: "compute.validate_phone".into(),
            arg_shape: "{v:str}".into(),
        },
        TraceStep {
            call_id: "tool.send_otp".into(),
            arg_shape: "{to:str}".into(),
        },
    ];
    let proc_obs: Vec<(String, bool)> = (0..20)
        .map(|i| {
            (
                format!("proc-{i}"),
                procedure_agreement(&predicted, &predicted),
            )
        })
        .collect();
    assert!(
        advances(&proc_obs),
        "a faithfully-predicted procedure advances (same gate)"
    );

    // A mispredicted procedure disagrees and does not advance.
    let wrong = vec![TraceStep {
        call_id: "tool.other".into(),
        arg_shape: "{}".into(),
    }];
    let bad_obs: Vec<(String, bool)> = (0..20)
        .map(|i| {
            (
                format!("proc-bad-{i}"),
                procedure_agreement(&predicted, &wrong),
            )
        })
        .collect();
    assert!(
        !advances(&bad_obs),
        "a mispredicting procedure is caught by the same gate"
    );
}

#[test]
fn evidence_cannot_be_inflated_by_repetition() {
    // 100 observations but only 5 distinct inputs → distinct count is 5, below the ≥20 bar (§13.2).
    let repeated: Vec<(String, bool)> = (0..100)
        .map(|i| (format!("input-{}", i % 5), true))
        .collect();
    assert!(
        !advances(&repeated),
        "repetition can't inflate distinct-input evidence (§13.2)"
    );
}
