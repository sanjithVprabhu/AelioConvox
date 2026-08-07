use aelio_agent::orchestration::suspension::{
    GraphSuspension, SuspendedExecutorSnapshot, DEFAULT_GRAPH_SUSPENSION_MAX_RETRIES,
};
use aelio_agent::orchestration::tool_router::single_tool_graph;
use aelio_agent::orchestration::{ExecutorState, ToolRoute};
use aelio_agent::tenant::OutputField;
use aelio_agent::types::Sensitivity;
use aelio_agent::World;
use indexmap::IndexMap;

fn pending_send_otp(world: &mut World, expires_at_ms: i64) -> GraphSuspension {
    let tool = world
        .registry
        .tools
        .get_mut("send_otp")
        .expect("demo client tool");
    tool.output_semantics.fields.insert(
        "ok".into(),
        OutputField {
            path: "ok".into(),
            type_name: "bool".into(),
            sensitivity: Sensitivity::None,
            meaning: "the OTP request was accepted".into(),
        },
    );
    let tool = tool.clone();
    let graph = single_tool_graph("send an OTP", &tool, ToolRoute::Client);
    let pending_node_id = graph.nodes[0].id.clone();
    let mut suspension = GraphSuspension::awaiting_info(
        "turn-open".into(),
        "user-1".into(),
        graph,
        IndexMap::new(),
        SuspendedExecutorSnapshot::from(&ExecutorState::default()),
        pending_node_id,
        vec!["phone".into()],
        1,
    );
    suspension.expires_at_ms = expires_at_ms;
    suspension
}

#[test]
fn invalid_answer_reclarifies_then_valid_answer_resumes_once_with_turn_linkage() {
    let mut world = World::demo_tenant("tenant-suspension");
    let suspension = pending_send_otp(&mut world, i64::MAX);
    world
        .user_graph_suspensions
        .insert("user-1".into(), suspension);

    let invalid =
        world.run_turn_on_channel_with_id("user-1", "not a phone number", "web", "turn-invalid");
    assert!(
        invalid.suspended,
        "invalid input must re-clarify; steps={:?}",
        invalid.steps
    );
    let retry = invalid.graph_suspension.as_ref().expect("retry suspension");
    assert_eq!(retry.retry_count, 1);
    assert_eq!(retry.max_retries, DEFAULT_GRAPH_SUSPENSION_MAX_RETRIES);
    assert!(
        !retry.slots.contains_key("phone"),
        "invalid input must not become a durable slot"
    );
    assert_eq!(world.tool_host.invocation_count(), Some(0));

    let resumed =
        world.run_turn_on_channel_with_id("user-1", "+919876543210", "web", "turn-resume");
    assert!(!resumed.suspended);
    assert!(
        resumed.steps.iter().any(|step| {
            step.name == "Orchestrate.ResumeLinked"
                && step.detail.contains("suspended_turn=turn-open")
                && step.detail.contains("resumed_turn=turn-resume")
        }),
        "resume must link both turns; steps={:?}",
        resumed.steps
    );
    assert_eq!(
        world.tool_host.invocation_count(),
        Some(1),
        "the resumed graph must invoke the client tool exactly once"
    );
    assert!(!world.user_graph_suspensions.contains_key("user-1"));
    assert!(world
        .effect_env
        .metrics
        .iter()
        .any(|(name, value)| name == "graph_suspension.resumed" && *value == 1.0));
}

#[test]
fn expired_suspension_is_discarded_and_turn_runs_fresh() {
    let mut world = World::demo_tenant("tenant-suspension-expiry");
    let suspension = pending_send_otp(&mut world, 1);
    world
        .user_graph_suspensions
        .insert("user-1".into(), suspension);

    let result = world.run_turn_on_channel_with_id(
        "user-1",
        "calculate the average of 10 20 30",
        "web",
        "turn-fresh",
    );

    assert!(result
        .steps
        .iter()
        .any(|step| step.name == "Orchestrate.Expired"));
    assert!(result
        .steps
        .iter()
        .any(|step| step.name == "Orchestrate.Execute"));
    assert!(!world.user_graph_suspensions.contains_key("user-1"));
    assert!(world
        .effect_env
        .metrics
        .iter()
        .any(|(name, value)| name == "graph_suspension.expired" && *value == 1.0));
}
