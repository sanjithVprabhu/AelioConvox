//! Live LLM decide over the diverse 10-harness sim catalog.
//!
//! Checks two things per turn:
//! 1. Choice quality (right kind / harness, with soft sets for OOD & ambiguous)
//! 2. Executability (spawned id is in catalog AND child actually ran)
//!
//! ```bash
//! set -a && source ../../.env && set +a
//! cargo test -p aelio-agent --features direct-provider-tests \
//!   --test conductor_catalog_sim_live -- --ignored --nocapture
//! ```

#![cfg(feature = "direct-provider-tests")]

use aelio_agent::harness::llm_decide::run_catalog_sim_turn_with_llm;
use aelio_agent::provider::{LlmProvider, OpenAiCompatibleProvider};
use aelio_kernel::{grade_sim_turn, sim_chat_script_diverse};

fn live_enabled() -> bool {
    std::env::var("OPENAI_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
        && std::env::var("AELIO_SKIP_LIVE_LLM").ok().as_deref() != Some("1")
}

fn live_provider() -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::from_env().expect("OPENAI_API_KEY required")
}

#[test]
#[ignore = "live network: requires OPENAI_API_KEY"]
fn live_llm_diverse_choices_are_executable() {
    if !live_enabled() {
        eprintln!("skip: OPENAI_API_KEY not set");
        return;
    }

    let mut provider = live_provider();
    let script = sim_chat_script_diverse();
    let mut choice_ok = 0usize;
    let mut exec_ok = 0usize;
    let mut fail = 0usize;

    for step in &script {
        let result = match run_catalog_sim_turn_with_llm(&mut provider, step.utterance) {
            Ok(r) => r,
            Err(e) => {
                // Fail-closed on phantom harness / execute errors counts as executable rejection
                // when the script expected OOD (no spawn). Otherwise it's a hard fail.
                let msg = format!("{e:?}");
                let ood_rejected = step.category == "ood"
                    && (msg.contains("not in catalog") || msg.contains("Policy"));
                if ood_rejected {
                    choice_ok += 1;
                    exec_ok += 1;
                    eprintln!(
                        "turn {:?} [ood] → provider tried invalid spawn; rejected OK ({msg})",
                        step.utterance
                    );
                } else {
                    fail += 1;
                    eprintln!(
                        "FAIL [{}] {} → error {e:?}",
                        step.category, step.utterance
                    );
                }
                continue;
            }
        };

        let grade = grade_sim_turn(step, &result);
        eprintln!(
            "turn [{}] {:?} → kind={} spawn={:?} reply={:?} choice={} exec={}",
            step.category,
            step.utterance,
            result.decision_kind,
            result.spawned_harness_id,
            result.reply_text,
            grade.choice_ok,
            grade.executable_ok
        );

        if grade.executable_ok {
            exec_ok += 1;
        } else {
            fail += 1;
            eprintln!("  exec error: {:?}", grade.executable_error);
        }
        if grade.choice_ok {
            choice_ok += 1;
        } else if grade.executable_ok {
            eprintln!(
                "  soft choice miss expect kind={} harness={:?}/{:?}",
                step.kind, step.harness_id, step.allowed_harnesses
            );
        } else {
            fail += 1;
        }
    }

    let n = script.len();
    eprintln!(
        "live diverse score: choice={choice_ok}/{n} exec={exec_ok}/{n} fail={fail} llm_calls={}",
        provider.calls().len()
    );
    assert_eq!(
        exec_ok, n,
        "every turn must be executable (spawn runs child, or reply is non-empty)"
    );
    assert!(
        choice_ok * 100 / n >= 75,
        "live Conductor choice agreement too low: {choice_ok}/{n}"
    );
    assert_eq!(fail, 0, "no hard failures allowed (exec or invalid decide)");
}
