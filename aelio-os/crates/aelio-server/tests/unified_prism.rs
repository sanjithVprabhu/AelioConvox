use std::sync::Arc;

use aelio_db_api::embed::Embedder;
use aelio_db_api::{AppState as DatabaseState, BoxFuture};
use aelio_db_query::Database;
use aelio_runtime::{Runtime, RuntimeConfig, DEFAULT_QUEUE_DEPTH};
use aelio_server::ServerState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

struct FixedEmbedder;

impl Embedder for FixedEmbedder {
    fn embed<'a>(&'a self, texts: Vec<String>) -> BoxFuture<'a, Result<Vec<Vec<f32>>, String>> {
        Box::pin(async move { Ok(texts.into_iter().map(|_| vec![0.0, 0.0]).collect()) })
    }
}

#[test]
fn tenant_scoped_database_rejects_duplicate_credential_authority() {
    let root = tempfile::tempdir().unwrap();
    let database = Database::create(root.path()).unwrap();
    let result = DatabaseState::new_tenant_scoped(
        database,
        vec![
            ("tenant-a".into(), "one-token-with-16-bytes".into()),
            ("tenant-b".into(), "one-token-with-16-bytes".into()),
        ],
    );
    assert!(result.is_err());
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = if let Some(body) = body {
        request
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    } else {
        request.body(Body::empty()).unwrap()
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn unified_authenticated_prism_covers_modalities_and_rejects_invalid_envelopes() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(RuntimeConfig {
        data_dir: root.path().join("runtime"),
        host_token: None,
        host_url: None,
        event_key_secret: [71; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap();
    let runtime_state =
        ServerState::new(runtime, vec!["prism-production-key".into()], false).unwrap();
    let database_dir = root.path().join("database");
    std::fs::create_dir_all(&database_dir).unwrap();
    let database = Database::create(database_dir).unwrap();
    let database_state = DatabaseState::new(database, vec!["prism-production-key".into()])
        .with_embedder(Arc::new(FixedEmbedder));
    let app = aelio_server::router(runtime_state).merge(aelio_db_api::router(database_state));

    let (status, _) = call(
        &app,
        "POST",
        "/v1/prism",
        None,
        Some(json!({"from":"docs","select":["id"],"limit":1})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, created) = call(
        &app,
        "POST",
        "/v1/tables",
        Some("prism-production-key"),
        Some(json!({"name":"docs","columns":[
            {"name":"id","kind":"utf8"},
            {"name":"body","kind":"text"},
            {"name":"embedding","kind":"vector","dim":2},
            {"name":"category","kind":"utf8"},
            {"name":"active","kind":"bool"},
            {"name":"links","kind":"edge"}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");

    let insert = |id: &str, body: &str, embedding: [f64; 2], category: &str| {
        json!({"values":{
            "id":{"type":"utf8","value":id},
            "body":{"type":"utf8","value":body},
            "embedding":{"type":"vector","value":embedding},
            "category":{"type":"utf8","value":category},
            "active":{"type":"bool","value":true},
            "links":{"type":"edges","value":[]}
        }})
    };
    let (status, first) = call(
        &app,
        "POST",
        "/v1/tables/docs/rows",
        Some("prism-production-key"),
        Some(insert("a", "customer account help", [0.0, 0.0], "support")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let first_id = first["row_id"].as_u64().unwrap();
    let (status, second) = call(
        &app,
        "POST",
        "/v1/tables/docs/rows",
        Some("prism-production-key"),
        Some(insert("b", "neighbor customer note", [1.0, 1.0], "support")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let second_id = second["row_id"].as_u64().unwrap();
    let (status, linked) = call(
        &app,
        "PATCH",
        &format!("/v1/tables/docs/rows/{first_id}"),
        Some("prism-production-key"),
        Some(json!({"values":{"links":{"type":"edges","value":[second_id]}}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{linked}");

    let valid = [
        (
            json!({
                "from":"docs",
                "where":[{"col":"category","op":"eq","value":"support"}],
                "select":["id"],"limit":10
            }),
            vec!["scalar"],
        ),
        (
            json!({
                "from":"docs",
                "match":{"kind":"text","on":"body","query":"customer"},
                "select":["id","body"],"limit":10
            }),
            vec!["text"],
        ),
        (
            json!({
                "from":"docs",
                "match":{"kind":"vector","on":"embedding","vector":[0.0,0.0]},
                "select":["id"],"limit":10
            }),
            vec!["vector"],
        ),
        (
            json!({
                "from":"docs",
                "match":{"kind":"graph","on":"links","seeds":[first_id],"max_depth":1,"max_nodes":10},
                "select":["id"],"limit":10
            }),
            vec!["graph"],
        ),
        (
            json!({
                "from":"docs",
                "match":[
                    {"kind":"text","on":"body","query":"customer"},
                    {"kind":"vector","on":"embedding","embed":"customer help"},
                    {"kind":"fusion","method":"rrf","rrf_k":60.0}
                ],
                "where":[{"col":"active","op":"eq","value":true}],
                "select":["id","body"],"limit":10
            }),
            vec!["fusion", "scalar", "text", "vector"],
        ),
    ];
    for (query, expected_modalities) in valid {
        let (status, response) = call(
            &app,
            "POST",
            "/v1/prism",
            Some("prism-production-key"),
            Some(query.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert!(
            !response["results"].as_array().unwrap().is_empty(),
            "{response}"
        );
        assert_eq!(response["decision"]["authority"], "aelio-prism");
        assert_eq!(response["decision"]["collection"], "docs");
        assert_eq!(
            response["decision"]["modalities"],
            json!(expected_modalities)
        );
        assert_eq!(response["decision"]["limit"], 10);
        assert_eq!(
            response["decision"]["result_count"].as_u64().unwrap() as usize,
            response["results"].as_array().unwrap().len()
        );
        assert!(response["results"].as_array().unwrap().iter().all(|hit| {
            hit["fields"].as_object().is_some_and(|fields| {
                !fields.contains_key("embedding")
                    && !fields.contains_key("category")
                    && !fields.contains_key("active")
                    && !fields.contains_key("links")
            })
        }));
        let (replay_status, replay) = call(
            &app,
            "POST",
            "/v1/prism",
            Some("prism-production-key"),
            Some(query),
        )
        .await;
        assert_eq!(replay_status, StatusCode::OK, "{replay}");
        assert_eq!(replay, response, "Prism read replay must be deterministic");
    }

    let invalid = [
        (
            json!({"from":"docs","select":["id"],"limit":1,"unknown":true}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"from":"docs","select":["id"],"limit":10001}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({
                "from":"docs",
                "match":{"kind":"vector","on":"embedding","vector":[0.0]},
                "select":["id"],"limit":1
            }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({
                "from":"docs",
                "match":{"kind":"graph","on":"links","seeds":[first_id],"max_depth":1,"max_nodes":1},
                "select":["id"],"limit":10
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ];
    for (query, expected) in invalid {
        let (status, response) = call(
            &app,
            "POST",
            "/v1/prism",
            Some("prism-production-key"),
            Some(query),
        )
        .await;
        assert_eq!(status, expected, "{response}");
        assert!(response["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()));
        assert!(response.get("results").is_none());
    }

    let (status, unchanged) = call(
        &app,
        "GET",
        &format!("/v1/tables/docs/rows/{first_id}"),
        Some("prism-production-key"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unchanged}");
    assert_eq!(unchanged["values"]["id"]["value"], "a");
    assert_eq!(unchanged["values"]["links"]["value"], json!([second_id]));
}

#[tokio::test]
async fn tenant_credentials_isolate_rows_modalities_artifacts_and_evidence() {
    const TOKEN_A: &str = "tenant-a-isolation-token";
    const TOKEN_B: &str = "tenant-b-isolation-token";
    let root = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(RuntimeConfig {
        data_dir: root.path().join("runtime"),
        host_token: None,
        host_url: None,
        event_key_secret: [72; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap();
    let runtime_state = ServerState::new_tenant_scoped(
        runtime,
        vec![
            ("tenant-a".into(), TOKEN_A.into()),
            ("tenant-b".into(), TOKEN_B.into()),
        ],
    )
    .unwrap();
    let database_dir = root.path().join("database");
    std::fs::create_dir_all(&database_dir).unwrap();
    let database = Database::create(database_dir).unwrap();
    let database_state = DatabaseState::new_tenant_scoped(
        database,
        vec![
            ("tenant-a".into(), TOKEN_A.into()),
            ("tenant-b".into(), TOKEN_B.into()),
        ],
    )
    .unwrap()
    .with_embedder(Arc::new(FixedEmbedder));
    let app = aelio_server::router(runtime_state).merge(aelio_db_api::router(database_state));

    let schema = json!({"name":"docs","columns":[
        {"name":"id","kind":"utf8"},
        {"name":"body","kind":"text"},
        {"name":"embedding","kind":"vector","dim":2},
        {"name":"category","kind":"utf8"},
        {"name":"links","kind":"edge"}
    ]});
    for token in [TOKEN_A, TOKEN_B] {
        let (status, created) = call(
            &app,
            "POST",
            "/v1/tables",
            Some(token),
            Some(schema.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
    }
    let row = |id: &str, body: &str, category: &str| {
        json!({"values":{
            "id":{"type":"utf8","value":id},
            "body":{"type":"utf8","value":body},
            "embedding":{"type":"vector","value":[0.0,0.0]},
            "category":{"type":"utf8","value":category},
            "links":{"type":"edges","value":[]}
        }})
    };
    let (status, a_row) = call(
        &app,
        "POST",
        "/v1/tables/docs/rows",
        Some(TOKEN_A),
        Some(row("a-private", "a-only-secret customer", "a-private")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{a_row}");
    let a_row_id = a_row["row_id"].as_u64().unwrap();
    let (status, b_row) = call(
        &app,
        "POST",
        "/v1/tables/docs/rows",
        Some(TOKEN_B),
        Some(row("b-public", "b-only-public customer", "b-public")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{b_row}");
    let b_row_id = b_row["row_id"].as_u64().unwrap();

    let attacks = [
        json!({
            "from":"docs","where":[{"col":"category","op":"eq","value":"a-private"}],
            "select":["id","body"],"limit":10
        }),
        json!({
            "from":"docs","match":{"kind":"text","on":"body","query":"a-only-secret"},
            "select":["id","body"],"limit":10
        }),
        json!({
            "from":"docs","match":{"kind":"vector","on":"embedding","vector":[0.0,0.0]},
            "select":["id","body"],"limit":10
        }),
        json!({
            "from":"docs","match":{"kind":"graph","on":"links","seeds":[a_row_id],"max_depth":1,"max_nodes":10},
            "select":["id","body"],"limit":10
        }),
        json!({
            "from":"docs","match":[
                {"kind":"text","on":"body","query":"a-only-secret"},
                {"kind":"vector","on":"embedding","vector":[0.0,0.0]},
                {"kind":"fusion","method":"rrf","rrf_k":60.0}
            ],"select":["id","body"],"limit":10
        }),
    ];
    for attack in attacks {
        let (status, response) = call(&app, "POST", "/v1/prism", Some(TOKEN_B), Some(attack)).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(!encoded.contains("a-only-secret"), "{response}");
        assert!(!encoded.contains("a-private"), "{response}");
    }
    let (status, b_get) = call(
        &app,
        "GET",
        &format!("/v1/tables/docs/rows/{a_row_id}"),
        Some(TOKEN_B),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{b_get}");
    let (status, own_b_get) = call(
        &app,
        "GET",
        &format!("/v1/tables/docs/rows/{b_row_id}"),
        Some(TOKEN_B),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{own_b_get}");
    assert_eq!(own_b_get["values"]["id"]["value"], "b-public");

    let flow = json!({
        "tenant":"tenant-a","flow_id":"flow.tenant-a","flow_rev":"1",
        "program":{"nid":"root","op":"Const","v":{"ok":true}},
        "targets":[],"prompts":[]
    });
    let (status, pushed) = call(&app, "POST", "/v1/flows", Some(TOKEN_A), Some(flow.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{pushed}");
    let gate_cases: Vec<_> = (0..20)
        .map(|index| {
            json!({
                "input":{"turn":{"index":index}},"wakes":[],"expect_park":false,
                "expected":{"ok":true},"fixtures":[]
            })
        })
        .collect();
    let (status, gated) = call(
        &app,
        "POST",
        "/v1/flows/gate",
        Some(TOKEN_A),
        Some(json!({"flow":flow,"cases":gate_cases})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{gated}");

    let (status, artifact_attack) = call(
        &app,
        "POST",
        "/v1/turns",
        Some(TOKEN_B),
        Some(json!({
            "tenant":"tenant-a","instance_id":"cross-tenant-artifact",
            "flow_id":"flow.tenant-a","flow_rev":"1","input":{"turn":{}}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{artifact_attack}");
    assert_eq!(artifact_attack["error"]["code"], "tenant_forbidden");
    let (status, evidence_attack) = call(
        &app,
        "POST",
        "/v1/artifacts/canary-evidence",
        Some(TOKEN_B),
        Some(json!({
            "tenant":"tenant-a","artifact_id":"flow.tenant-a","artifact_version":1,
            "observation":{
                "input_hash":"a".repeat(64),"downstream_success":true,
                "guard_violation":false,"ledger_hash":"b".repeat(64)
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{evidence_attack}");
    assert_eq!(evidence_attack["error"]["code"], "tenant_forbidden");

    let (status, own_turn) = call(
        &app,
        "POST",
        "/v1/turns",
        Some(TOKEN_A),
        Some(json!({
            "tenant":"tenant-a","instance_id":"tenant-a-own-artifact",
            "flow_id":"flow.tenant-a","flow_rev":"1","input":{"turn":{}}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{own_turn}");
    assert_eq!(own_turn["bag"]["ok"], true);
}
