//! Phase D — Conductor + harness stack routing (HARNESS_CONDUCTOR_VISION.md §14.3).

use aelio_agent::runtime::World;
use aelio_agent::{detect_stack_control, select_starter_harness, HarnessSession, StackControl, StarterHarness};

fn step_named(result: &aelio_agent::blocks::turn::TurnResult, name: &str) -> bool {
    result.steps.iter().any(|s| s.name == name)
}

fn step_detail_contains(result: &aelio_agent::blocks::turn::TurnResult, name: &str, needle: &str) -> bool {
    result
        .steps
        .iter()
        .any(|s| s.name == name && s.detail.contains(needle))
}

#[test]
fn empty_stack_runs_conductor_then_quick_reply_on_hi() {
    let mut world = World::demo_tenant("harness-hi");
    let r = world.run_turn("u1", "Hi");
    assert!(step_named(&r, "Conductor") || step_named(&r, "Conductor.Select"));
    assert!(step_named(&r, "Harness.quick_reply") || step_detail_contains(&r, "Conductor.Select", "quick_reply"));
    assert!(!step_named(&r, "ProposePath"), "greeting must not cold-propose");
    assert_eq!(r.llm_calls, 0);
    assert!(!world.user_harness["u1"].stack.is_empty());
    assert_eq!(
        world.user_harness["u1"].top().map(|f| f.harness_id.as_str()),
        Some("conductor")
    );
}

#[test]
fn conductor_selects_understand_intent_for_help_me() {
    let mut world = World::demo_tenant("harness-intent");
    let r = world.run_turn("u1", "help me figure out what to do next");
    assert!(step_detail_contains(&r, "Conductor.Select", "understand_intent"));
    assert!(step_named(&r, "Harness.understand_intent"));
    assert!(!step_named(&r, "ProposePath"));
}

#[test]
fn wait_for_user_owns_next_message_until_resume() {
    let mut world = World::demo_tenant("harness-wait");
    let parked = world.run_turn("u1", "please ask me what you need");
    assert!(step_named(&parked, "Harness.wait_for_user"));
    assert!(parked.suspended || parked.opened_loop);
    assert_eq!(
        world.user_harness["u1"]
            .waiting_child()
            .map(|f| f.harness_id.as_str()),
        Some("wait_for_user")
    );

    let resumed = world.run_turn("u1", "I need a short summary");
    assert!(step_named(&resumed, "Harness.wait_for_user.resume"));
    assert!(world.user_harness["u1"].waiting_child().is_none());
    assert!(
        world.user_harness["u1"]
            .pages
            .get("conductor")
            .is_some_and(|p| !p.notes.is_empty()),
        "context page should retain notes across park/resume"
    );
}

#[test]
fn exit_up_pops_one_layer_only() {
    let mut session = HarnessSession::default();
    session.push("conductor", false);
    session.push("wait_for_user", true);
    session.push("child", true);
    assert_eq!(detect_stack_control("go back"), StackControl::ExitUp);
    let popped = session.exit_up().unwrap();
    assert_eq!(popped.harness_id, "child");
    assert_eq!(session.top().unwrap().harness_id, "wait_for_user");
}

#[test]
fn fresh_clears_to_empty_ceiling() {
    let mut world = World::demo_tenant("harness-fresh");
    let _ = world.run_turn("u1", "please ask me what you need");
    assert!(world.user_harness["u1"].waiting_child().is_some());
    let fresh = world.run_turn("u1", "start over");
    assert!(step_detail_contains(&fresh, "Harness.Stack", "fresh"));
    // After fresh, Conductor re-pushes on the same turn when selecting a starter.
    assert!(
        world.user_harness["u1"]
            .top()
            .is_some_and(|f| f.harness_id == "conductor")
            || world.user_harness["u1"].is_empty()
    );
    assert!(world.user_harness["u1"].waiting_child().is_none());
}

#[test]
fn rule_select_matches_starter_catalog() {
    let session = HarnessSession::default();
    assert_eq!(select_starter_harness("hello", &session), StarterHarness::QuickReply);
    assert_eq!(
        select_starter_harness("help me figure out what to do next", &session),
        StarterHarness::UnderstandIntent
    );
    assert_eq!(
        select_starter_harness("send otp to 9611266596", &session),
        StarterHarness::Escalate
    );
}
