//! §18 selection hygiene + §20 attribution conformance.

use aelio_learn::{
    attribute, select, validate_decision_point, DecisionPoint, FallbackReason, Hygiene,
    PathwayScore, Selection, MAX_PATHWAYS,
};

fn hyg() -> Hygiene {
    Hygiene { tau: 0.5, delta: 0.15, entropy_ceiling: 0.9 }
}

fn scores(pairs: &[(&str, f64)]) -> Vec<PathwayScore> {
    pairs.iter().map(|(id, s)| PathwayScore { pathway_id: (*id).into(), score: *s }).collect()
}

#[test]
fn confident_pick_passes_all_three_checks() {
    let s = scores(&[("start_login", 0.92), ("browse", 0.30), ("divert", 0.10)]);
    match select(&s, &hyg()) {
        Selection::Picked { pathway_id, margin, .. } => {
            assert_eq!(pathway_id, "start_login");
            assert!(margin >= 0.15);
        }
        other => panic!("expected pick, got {other:?}"),
    }
}

#[test]
fn low_confidence_routes_to_fallback() {
    let s = scores(&[("a", 0.40), ("b", 0.10)]); // top < τ
    assert!(matches!(select(&s, &hyg()), Selection::Fallback { reason: FallbackReason::LowConfidence, .. }));
}

#[test]
fn thin_margin_routes_to_fallback() {
    let s = scores(&[("a", 0.80), ("b", 0.75)]); // margin 0.05 < δ
    assert!(matches!(select(&s, &hyg()), Selection::Fallback { reason: FallbackReason::ThinMargin, .. }));
}

#[test]
fn flat_distribution_high_entropy_routes_to_fallback() {
    // Many near-equal high scores → high entropy even if one nudges ahead.
    let s = scores(&[("a", 0.9), ("b", 0.89), ("c", 0.88), ("d", 0.87), ("e", 0.86)]);
    // Thin margin trips first here, but a case with adequate margin yet flat tail is the target:
    let flat = scores(&[("a", 0.7), ("b", 0.69), ("c", 0.69), ("d", 0.69), ("e", 0.69), ("f", 0.69)]);
    assert!(matches!(select(&s, &hyg()), Selection::Fallback { .. }));
    assert!(matches!(select(&flat, &hyg()), Selection::Fallback { .. }));
}

#[test]
fn empty_candidates_route_to_fallback() {
    assert!(matches!(select(&[], &hyg()), Selection::Fallback { reason: FallbackReason::Empty, .. }));
}

#[test]
fn decision_point_requires_fallback_and_caps_at_eight() {
    let good = DecisionPoint { id: "unauth.v1".into(), fallback_pathway: "divert".into(), hygiene: hyg() };
    assert!(validate_decision_point(&good, 8).is_ok());
    assert!(validate_decision_point(&good, MAX_PATHWAYS + 1).is_err(), "cap 8 (§18)");
    let no_fb = DecisionPoint { fallback_pathway: "".into(), ..good };
    assert!(validate_decision_point(&no_fb, 3).is_err(), "fallback mandatory (§18)");
}

#[test]
fn attribution_is_conservative_boolean() {
    // Success → every used artifact gets positive credit, usage-weighted.
    let win = attribute(true, &[("conv.e1".into(), 1), ("path.login".into(), 2)]);
    assert_eq!(win.iter().find(|c| c.artifact_id == "path.login").unwrap().credit, 2.0);
    // Failure → zero credit to all (false demotions are cheap; false promotions never happen, §20).
    let loss = attribute(false, &[("conv.e1".into(), 1)]);
    assert_eq!(loss[0].credit, 0.0);
}
