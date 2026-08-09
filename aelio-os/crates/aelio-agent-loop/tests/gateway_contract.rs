use aelio_agent_loop::{AgentModelRequestV2, AgentModelResponseV2, ToolChoiceKindV2};
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/harness_new/fixtures")
        .join(name);
    std::fs::read_to_string(path).expect("fixture is readable")
}

#[test]
fn model_gateway_v2_fixtures_parse_strictly_in_rust() {
    let request: AgentModelRequestV2 =
        serde_json::from_str(&fixture("model_gateway_v2_request.json")).expect("valid request");
    assert_eq!(request.protocol_version, 2);
    assert_eq!(request.tool_choice.kind, ToolChoiceKindV2::Auto);
    assert_eq!(request.tools[0].name, "finish");

    let response: AgentModelResponseV2 =
        serde_json::from_str(&fixture("model_gateway_v2_response.json")).expect("valid response");
    assert_eq!(response.request_id, request.request_id);
    assert_eq!(response.attempt_id, request.attempt_id);
}

#[test]
fn model_gateway_v2_rejects_unknown_fields_in_rust() {
    let mut value: Value =
        serde_json::from_str(&fixture("model_gateway_v2_request.json")).expect("json fixture");
    value
        .as_object_mut()
        .expect("request object")
        .insert("unexpected".to_string(), Value::Bool(true));
    assert!(serde_json::from_value::<AgentModelRequestV2>(value).is_err());
}
