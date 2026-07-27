//! §26 pathway explainer: the human-readable form of the pathway_pick decision.

use aelio_learn::{explain, Hygiene, PathwayScore};

fn hygiene() -> Hygiene {
    Hygiene {
        tau: 0.5,
        delta: 0.1,
        entropy_ceiling: 0.9,
    }
}

#[test]
fn explain_shows_scores_and_the_pick() {
    let cands = vec![
        PathwayScore {
            pathway_id: "greet".into(),
            score: 0.82,
        },
        PathwayScore {
            pathway_id: "escalate".into(),
            score: 0.30,
        },
    ];
    let text = explain(&cands, &hygiene());
    assert!(text.contains("greet"));
    assert!(text.contains("escalate"));
    assert!(
        text.contains("→ PICKED greet"),
        "explains the winner: {text}"
    );
    assert!(text.contains("margin="));
}

#[test]
fn explain_names_the_fallback_reason() {
    // Two near-equal scores → thin margin → fallback, with the reason spelled out.
    let cands = vec![
        PathwayScore {
            pathway_id: "a".into(),
            score: 0.71,
        },
        PathwayScore {
            pathway_id: "b".into(),
            score: 0.70,
        },
    ];
    let text = explain(&cands, &hygiene());
    assert!(text.contains("→ FALLBACK"), "{text}");
    assert!(
        text.contains("margin below δ"),
        "names the thin-margin reason: {text}"
    );
}
