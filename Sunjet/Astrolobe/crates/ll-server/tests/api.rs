//! HTTP-level integration tests, driving the router in-process via `tower::oneshot` (no real
//! socket). Exercises auth, the table/row lifecycle, and a hybrid query end to end.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ll_query::Database;
use ll_server::embed::Embedder;
use ll_server::nl::{BoxFuture, LlmClient};
use ll_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

fn app_with_keys(tag: &str, keys: Vec<String>) -> Router {
    router(AppState::new(fresh_db(tag), keys))
}

fn fresh_db(tag: &str) -> Database {
    let dir = std::env::temp_dir().join(format!("ll_api_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Database::create(&dir).unwrap()
}

/// A canned LLM backend: returns a fixed structured query regardless of the prompt — so the
/// NL endpoint's compile-and-execute path is testable without a network call.
struct MockLlm(Value);

impl LlmClient for MockLlm {
    fn translate<'a>(&'a self, _system: String, _user: String, _schema: Value) -> BoxFuture<'a, Result<Value, String>> {
        let v = self.0.clone();
        Box::pin(async move { Ok(v) })
    }
}

/// A deterministic embedder: every text maps to the same fixed 2-d vector — enough to test
/// that the `semantic`/`embed` paths embed-then-search without a network call.
struct MockEmbedder(Vec<f32>);

impl Embedder for MockEmbedder {
    fn embed<'a>(&'a self, texts: Vec<String>) -> BoxFuture<'a, Result<Vec<Vec<f32>>, String>> {
        let v = self.0.clone();
        Box::pin(async move { Ok(texts.iter().map(|_| v.clone()).collect()) })
    }
}

/// Issue one request and return `(status, json_body)`.
async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(k) = key {
        req = req.header("authorization", format!("Bearer {k}"));
    }
    let req = if let Some(b) = body {
        req.header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        req.body(Body::empty()).unwrap()
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

#[tokio::test]
async fn health_is_open_but_api_requires_a_key() {
    let app = app_with_keys("auth", vec!["secret".into()]);

    let (status, body) = call(&app, "GET", "/v1/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");

    // No key → 401 on the API surface.
    let (status, _) = call(&app, "POST", "/v1/tables", None, Some(json!({"name":"t","columns":[]}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Wrong key → 401.
    let (status, _) = call(
        &app,
        "POST",
        "/v1/tables",
        Some("nope"),
        Some(json!({"name":"t","columns":[]})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn table_row_lifecycle_and_hybrid_query() {
    let app = app_with_keys("life", vec![]); // open mode
    let k = None;

    // Create a table with all four modalities represented.
    let (status, body) = call(
        &app,
        "POST",
        "/v1/tables",
        k,
        Some(json!({"name":"docs","columns":[
            {"name":"embedding","kind":"vector","dim":2},
            {"name":"body","kind":"text"},
            {"name":"year","kind":"i64"}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create_table: {body}");

    // Two rows: one matches a 'diffusion' + recent filter, one does not.
    let (s1, b1) = call(
        &app,
        "POST",
        "/v1/tables/docs/rows",
        k,
        Some(json!({"values":{
            "embedding":{"type":"vector","value":[0.0,0.0]},
            "body":{"type":"utf8","value":"alpha diffusion model"},
            "year":{"type":"i64","value":2021}
        }})),
    )
    .await;
    assert_eq!(s1, StatusCode::OK);
    let id1 = b1["row_id"].as_u64().unwrap();

    let (_s2, _b2) = call(
        &app,
        "POST",
        "/v1/tables/docs/rows",
        k,
        Some(json!({"values":{
            "embedding":{"type":"vector","value":[10.0,0.0]},
            "body":{"type":"utf8","value":"beta survey"},
            "year":{"type":"i64","value":2015}
        }})),
    )
    .await;

    // Hybrid query: vector + text + scalar filter. Only row 1 qualifies.
    let (status, body) = call(
        &app,
        "POST",
        "/v1/tables/docs/query",
        k,
        Some(json!({
            "k":5,
            "vector":{"col":"embedding","query":[0.0,0.0]},
            "text":{"col":"body","query":"diffusion"},
            "filters":[{"col":"year","op":"ge","value":{"type":"i64","value":2019}}]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "query: {body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "expected only the qualifying row: {body}");
    assert_eq!(results[0]["row_id"].as_u64().unwrap(), id1);

    // Delete it; a second delete reports applied=false.
    let (_s, b) = call(&app, "DELETE", &format!("/v1/tables/docs/rows/{id1}"), k, None).await;
    assert_eq!(b["applied"], true);
    let (_s, b) = call(&app, "DELETE", &format!("/v1/tables/docs/rows/{id1}"), k, None).await;
    assert_eq!(b["applied"], false);
}

#[tokio::test]
async fn nl_endpoint_compiles_and_runs_a_query() {
    // The mock "model" translates English into this structured query.
    let mock = json!({
        "k": 5,
        "text": { "col": "body", "query": "diffusion" },
        "filters": [ { "col": "year", "op": "ge", "value": { "type": "i64", "value": 2019 } } ]
    });
    let state = AppState::new(fresh_db("nl"), vec![]).with_llm(Arc::new(MockLlm(mock.clone())));
    let app = router(state);
    let k = None;

    // Set up the table + a matching and a non-matching row.
    call(
        &app,
        "POST",
        "/v1/tables",
        k,
        Some(json!({"name":"docs","columns":[{"name":"body","kind":"text"},{"name":"year","kind":"i64"}]})),
    )
    .await;
    let (_s, b1) = call(&app, "POST", "/v1/tables/docs/rows", k, Some(json!({"values":{"body":{"type":"utf8","value":"a diffusion paper"},"year":{"type":"i64","value":2022}}}))).await;
    let id1 = b1["row_id"].as_u64().unwrap();
    call(&app, "POST", "/v1/tables/docs/rows", k, Some(json!({"values":{"body":{"type":"utf8","value":"old survey"},"year":{"type":"i64","value":2010}}}))).await;

    let (status, body) = call(
        &app,
        "POST",
        "/v1/tables/docs/nl",
        k,
        Some(json!({"query": "diffusion papers since 2019"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "nl: {body}");
    // The compiled query is echoed back for transparency...
    assert_eq!(body["compiled"], mock);
    // ...and only the qualifying row is returned.
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "nl results: {body}");
    assert_eq!(results[0]["row_id"].as_u64().unwrap(), id1);
}

#[tokio::test]
async fn semantic_clause_embeds_then_searches() {
    // The embedder maps any text to [1,0]; so a semantic query should rank the row at [1,0]
    // ahead of the row at [10,0], without the client ever sending a vector.
    let state = AppState::new(fresh_db("sem"), vec![]).with_embedder(Arc::new(MockEmbedder(vec![1.0, 0.0])));
    let app = router(state);
    let k = None;

    call(&app, "POST", "/v1/tables", k, Some(json!({"name":"docs","columns":[{"name":"emb","kind":"vector","dim":2}]}))).await;
    let (_s, near) = call(&app, "POST", "/v1/tables/docs/rows", k, Some(json!({"values":{"emb":{"type":"vector","value":[1.0,0.0]}}}))).await;
    let id_near = near["row_id"].as_u64().unwrap();
    call(&app, "POST", "/v1/tables/docs/rows", k, Some(json!({"values":{"emb":{"type":"vector","value":[10.0,0.0]}}}))).await;

    let (status, body) = call(
        &app,
        "POST",
        "/v1/tables/docs/query",
        k,
        Some(json!({"k":1, "semantic": {"col":"emb","text":"whatever the user typed"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "semantic: {body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results[0]["row_id"].as_u64().unwrap(), id_near, "semantic search ranked wrong row: {body}");
}

#[tokio::test]
async fn embed_on_insert_stores_a_vector() {
    let state = AppState::new(fresh_db("emb_ins"), vec![]).with_embedder(Arc::new(MockEmbedder(vec![1.0, 0.0])));
    let app = router(state);
    let k = None;

    call(&app, "POST", "/v1/tables", k, Some(json!({"name":"docs","columns":[{"name":"emb","kind":"vector","dim":2}]}))).await;
    // Insert with text-to-embed instead of a raw vector.
    let (status, b) = call(
        &app,
        "POST",
        "/v1/tables/docs/rows",
        k,
        Some(json!({"values":{"emb":{"type":"embed","value":"some document text"}}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "embed-insert: {b}");
    let id = b["row_id"].as_u64().unwrap();

    // It was embedded to [1,0]; a vector query at [1,0] finds it.
    let (_s, body) = call(&app, "POST", "/v1/tables/docs/query", k, Some(json!({"k":1,"vector":{"col":"emb","query":[1.0,0.0]}}))).await;
    assert_eq!(body["results"][0]["row_id"].as_u64().unwrap(), id, "embedded row not found: {body}");
}

#[tokio::test]
async fn semantic_dimension_mismatch_is_400() {
    // Embedder produces 2-d vectors, but the column is declared 3-d → clear 400, not a silent
    // empty result.
    let state = AppState::new(fresh_db("semdim"), vec![]).with_embedder(Arc::new(MockEmbedder(vec![1.0, 0.0])));
    let app = router(state);
    call(&app, "POST", "/v1/tables", None, Some(json!({"name":"d","columns":[{"name":"e","kind":"vector","dim":3}]}))).await;
    let (status, body) = call(&app, "POST", "/v1/tables/d/query", None, Some(json!({"k":1,"semantic":{"col":"e","text":"x"}}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("3-dimensional"), "unhelpful error: {body}");
}

#[tokio::test]
async fn embed_on_insert_dimension_mismatch_is_400() {
    let state = AppState::new(fresh_db("insdim"), vec![]).with_embedder(Arc::new(MockEmbedder(vec![1.0, 0.0])));
    let app = router(state);
    call(&app, "POST", "/v1/tables", None, Some(json!({"name":"d","columns":[{"name":"e","kind":"vector","dim":3}]}))).await;
    let (status, _) = call(&app, "POST", "/v1/tables/d/rows", None, Some(json!({"values":{"e":{"type":"embed","value":"x"}}}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn semantic_without_embedder_is_503() {
    let app = app_with_keys("sem503", vec![]);
    call(&app, "POST", "/v1/tables", None, Some(json!({"name":"d","columns":[{"name":"e","kind":"vector","dim":2}]}))).await;
    let (status, _) = call(&app, "POST", "/v1/tables/d/query", None, Some(json!({"k":1,"semantic":{"col":"e","text":"x"}}))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn nl_is_503_when_no_backend_configured() {
    let app = app_with_keys("nloff", vec![]); // no .with_llm
    call(&app, "POST", "/v1/tables", None, Some(json!({"name":"t","columns":[{"name":"b","kind":"text"}]}))).await;
    let (status, _) = call(&app, "POST", "/v1/tables/t/nl", None, Some(json!({"query":"x"}))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn schema_endpoint_describes_columns() {
    let app = app_with_keys("schema", vec![]);
    call(
        &app,
        "POST",
        "/v1/tables",
        None,
        Some(json!({"name":"docs","columns":[{"name":"emb","kind":"vector","dim":3},{"name":"body","kind":"text"}]})),
    )
    .await;
    let (status, body) = call(&app, "GET", "/v1/tables/docs/schema", None, None).await;
    assert_eq!(status, StatusCode::OK, "schema: {body}");
    let cols = body["columns"].as_array().unwrap();
    assert_eq!(cols.len(), 2);
    let emb = cols.iter().find(|c| c["name"] == "emb").unwrap();
    assert_eq!(emb["kind"], "vector");
    assert_eq!(emb["dim"], 3);
}

#[tokio::test]
async fn unknown_table_is_404_and_bad_kind_is_400() {
    let app = app_with_keys("errs", vec![]);
    let k = None;

    let (status, _) = call(
        &app,
        "POST",
        "/v1/tables/ghost/query",
        k,
        Some(json!({"k":1,"vector":{"col":"v","query":[0.0]}})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = call(
        &app,
        "POST",
        "/v1/tables",
        k,
        Some(json!({"name":"t","columns":[{"name":"v","kind":"vector"}]})), // missing dim
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
