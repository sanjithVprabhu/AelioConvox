//! §19 procedure mining + §21 metrics / kill-criteria conformance.

use aelio_learn::{
    converter_falsified, mine, procedure_demoted_to_experimental, ConverterMetrics, FlowTrace,
    KillCriteria, MiningParams, ProcedureMetrics, TraceStep,
};

fn step(id: &str, shape: &str) -> TraceStep {
    TraceStep {
        call_id: id.into(),
        arg_shape: shape.into(),
    }
}

fn trace(steps: &[(&str, &str)], success: bool, day: u32) -> FlowTrace {
    FlowTrace {
        steps: steps.iter().map(|(i, s)| step(i, s)).collect(),
        success,
        day,
    }
}

#[test]
fn mines_recurring_successful_subsequence() {
    // validate_phone → send_otp recurs in 6 successful in-window flows.
    let common = &[
        ("compute.validate_phone", "{v:str}"),
        ("tool.send_otp", "{to:str}"),
    ][..];
    let mut traces: Vec<FlowTrace> = (0..6)
        .map(|d| {
            let mut steps = vec![("io.state_read", "{key:str}")];
            steps.extend_from_slice(common);
            steps.push(("model.express_success", "{}"));
            trace(&steps, true, d)
        })
        .collect();
    // A failed flow with the same subsequence must NOT count (§19: successful occurrences only).
    traces.push(trace(
        &[
            ("compute.validate_phone", "{v:str}"),
            ("tool.send_otp", "{to:str}"),
        ],
        false,
        1,
    ));

    let candidates = mine(&traces, &MiningParams::default(), 10);
    assert!(
        candidates.iter().any(|c| c.signature
            == vec![
                step("compute.validate_phone", "{v:str}"),
                step("tool.send_otp", "{to:str}")
            ]
            && c.occurrences >= 5),
        "the recurring validate→send subsequence is proposed (§19)"
    );
}

#[test]
fn rare_or_out_of_window_subsequences_are_not_mined() {
    // Only 4 occurrences → below the ≥5 threshold.
    let rare: Vec<FlowTrace> = (0..4)
        .map(|d| trace(&[("tool.a", "{}"), ("tool.b", "{}")], true, d))
        .collect();
    assert!(
        mine(&rare, &MiningParams::default(), 10).is_empty(),
        "≥5 threshold (§19)"
    );

    // 6 occurrences but all outside the 30-day window.
    let stale: Vec<FlowTrace> = (0..6)
        .map(|_| trace(&[("tool.a", "{}"), ("tool.b", "{}")], true, 0))
        .collect();
    assert!(
        mine(&stale, &MiningParams::default(), 200).is_empty(),
        "30-day window (§19)"
    );
}

#[test]
fn arg_shape_isomorphism_distinguishes_procedures() {
    // Same call id but different arg shapes are NOT the same procedure.
    let mixed: Vec<FlowTrace> = (0..6)
        .map(|d| {
            if d % 2 == 0 {
                trace(&[("tool.x", "{a:int}"), ("tool.y", "{}")], true, d)
            } else {
                trace(&[("tool.x", "{a:str}"), ("tool.y", "{}")], true, d)
            }
        })
        .collect();
    // Neither shape variant reaches 5 on its own → nothing mined at length 2 for tool.x pairs.
    let cands = mine(&mixed, &MiningParams::default(), 10);
    assert!(
        !cands
            .iter()
            .any(|c| c.signature.first().map(|s| s.call_id.as_str()) == Some("tool.x")),
        "different arg shapes are different procedures (§19 compatible-shape rule)"
    );
}

#[test]
fn converter_kill_criterion() {
    let kc = KillCriteria::default();
    // Below 60% at 90+ days ⇒ falsified.
    let bad = ConverterMetrics {
        warm_hits: 50,
        total_lookups: 100,
        days_observed: 95,
    };
    assert!(converter_falsified(&bad, &kc));
    // Above the floor ⇒ not falsified.
    let good = ConverterMetrics {
        warm_hits: 70,
        total_lookups: 100,
        days_observed: 95,
    };
    assert!(!converter_falsified(&good, &kc));
    // Before the window ⇒ not yet judged.
    let early = ConverterMetrics {
        warm_hits: 10,
        total_lookups: 100,
        days_observed: 30,
    };
    assert!(!converter_falsified(&early, &kc));
}

#[test]
fn procedure_kill_criterion() {
    let kc = KillCriteria::default();
    // < 10% touch at 6mo across ≥3 tenants ⇒ demoted to experimental.
    let bad = ProcedureMetrics {
        turns_touching_a_procedure: 5,
        total_turns: 100,
        days_observed: 190,
        active_tenants: 3,
    };
    assert!(procedure_demoted_to_experimental(&bad, &kc));
    // Only 2 tenants ⇒ not enough evidence to judge.
    let thin = ProcedureMetrics {
        active_tenants: 2,
        ..bad
    };
    assert!(!procedure_demoted_to_experimental(&thin, &kc));
}
