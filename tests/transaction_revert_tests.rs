mod common;

use async_trait::async_trait;
use axum::http::StatusCode;
use financetracker::embeddings::EmbeddingProvider;
use financetracker::{AppState, build_app};
use http_body_util::BodyExt;
use pgvector::Vector;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;
use tower::util::ServiceExt;

struct TestProvider {
    calls: AtomicUsize,
    fail_second: bool,
    block_second: bool,
    started: Notify,
    release: Notify,
}

impl TestProvider {
    fn new(fail_second: bool, block_second: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            fail_second,
            block_second,
            started: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for TestProvider {
    async fn generate_embedding(
        &self,
        _: &reqwest::Client,
        _: &str,
        text: &str,
    ) -> Result<Vec<f32>, (StatusCode, String)> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == 2 && self.block_second {
            self.started.notify_one();
            self.release.notified().await;
        }
        if call == 2 && self.fail_second {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "simulated provider failure".to_string(),
            ));
        }
        let mut vector = vec![0.0; 1536];
        vector[if text.contains("Original") { 0 } else { 1 }] = 1.0;
        Ok(vector)
    }
}

fn request(
    token: &str,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

async fn fixture(
    provider: Arc<TestProvider>,
) -> (AppState, axum::Router, uuid::Uuid, String, uuid::Uuid) {
    let state = common::setup_app_state_with_embedding_provider(provider).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, token) = common::login_test_user(&app, &username, &password).await;
    common::add_transaction(&app, &token, serde_json::json!({
        "amount": "75.50", "kind": "Income", "category": "Edited", "description": "Edited description", "date": "2026-03-12"
    })).await;
    let transaction_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM transactions WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    (state, app, user_id, token, transaction_id)
}

async fn attach_original(
    state: &AppState,
    user_id: uuid::Uuid,
    transaction_id: uuid::Uuid,
    kind: &str,
    category: Option<&str>,
    description: Option<&str>,
) {
    let connection_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO bank_connections (user_id, provider, provider_id, encrypted_access_token)
         VALUES ($1, 'test', $2, $3) RETURNING connection_id",
    )
    .bind(user_id)
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(vec![1u8])
    .fetch_one(&state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO imported_transaction_details
         (transaction_id, connection_id, provider, provider_transaction_id, provider_account_id, source_status, original_payload, inferred_kind, inferred_category, inferred_description)
         VALUES ($1, $2, 'test', 'provider-transaction', 'account', 'posted', $3, $4, $5, $6)"
    ).bind(transaction_id).bind(connection_id).bind(serde_json::json!({"raw": "immutable", "amount": 50, "date": "2026-03-01"}))
        .bind(kind).bind(category).bind(description).execute(&state.pool).await.unwrap();
}

async fn list(app: &axum::Router, token: &str) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(request(
            token,
            "GET",
            "/api/transactions",
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json(response).await
}

async fn revert(app: &axum::Router, token: &str, id: uuid::Uuid) -> axum::response::Response {
    app.clone()
        .oneshot(request(
            token,
            "POST",
            &format!("/api/transactions/{}/revert", id),
            serde_json::Value::Null,
        ))
        .await
        .unwrap()
}

#[tokio::test]
async fn revert_restores_imported_fields_and_preserves_money_date_and_source() {
    let provider = Arc::new(TestProvider::new(false, false));
    let (state, app, user_id, token, id) = fixture(provider.clone()).await;
    attach_original(
        &state,
        user_id,
        id,
        "expense",
        Some("Transportation"),
        Some("Original commute"),
    )
    .await;
    assert_eq!(list(&app, &token).await[0]["can_revert"], true);
    let source_before: serde_json::Value = sqlx::query_scalar(
        "SELECT to_jsonb(d) FROM imported_transaction_details d WHERE transaction_id = $1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let response = revert(&app, &token, id).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json(response).await,
        serde_json::json!({"kind": "Expense", "category": "Transportation", "description": "Original commute"})
    );
    let transactions = list(&app, &token).await;
    assert_eq!(
        transactions[0]["amount"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::new(7550, 2)
    );
    assert_eq!(transactions[0]["date"], "2026-03-12");
    assert_eq!(transactions[0]["kind"], "Expense");
    assert_eq!(transactions[0]["category"], "Transportation");
    assert_eq!(transactions[0]["description"], "Original commute");
    assert_eq!(transactions[0]["can_revert"], false);
    let source_after: serde_json::Value = sqlx::query_scalar(
        "SELECT to_jsonb(d) FROM imported_transaction_details d WHERE transaction_id = $1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(source_before, source_after);
    let (text, vector): (String, Vector) = sqlx::query_as(
        "SELECT embedding_text, embedding FROM transaction_embeddings WHERE transaction_id = $1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert!(text.contains("Original commute"));
    assert_eq!(vector.to_vec()[0], 1.0);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    // A repeated restore is harmless and does not spend another provider call.
    assert_eq!(revert(&app, &token, id).await.status(), StatusCode::OK);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn revert_restores_null_optional_fields() {
    let (state, app, user_id, token, id) = fixture(Arc::new(TestProvider::new(false, false))).await;
    attach_original(&state, user_id, id, "income", None, None).await;
    assert_eq!(revert(&app, &token, id).await.status(), StatusCode::OK);
    let transactions = list(&app, &token).await;
    assert_eq!(transactions[0]["category"], serde_json::Value::Null);
    assert_eq!(transactions[0]["description"], serde_json::Value::Null);
    assert_eq!(transactions[0]["can_revert"], false);
}

#[tokio::test]
async fn manual_missing_and_other_users_transactions_cannot_be_reverted() {
    let provider = Arc::new(TestProvider::new(false, false));
    let (state, app, user_id, token, id) = fixture(provider.clone()).await;
    let before = list(&app, &token).await;
    assert_eq!(before[0]["can_revert"], false);
    assert_eq!(
        revert(&app, &token, id).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        revert(&app, &token, uuid::Uuid::new_v4()).await.status(),
        StatusCode::NOT_FOUND
    );
    attach_original(&state, user_id, id, "expense", None, Some("Original")).await;
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (_, other_token) = common::login_test_user(&app, &username, &password).await;
    assert_eq!(
        revert(&app, &other_token, id).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(list(&app, &other_token).await, serde_json::json!([]));
    assert_eq!(
        list(&app, &token).await[0]["description"],
        "Edited description"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn revert_requires_bank_connection_ownership_too() {
    let provider = Arc::new(TestProvider::new(false, false));
    let (state, app, _, token, id) = fixture(provider.clone()).await;
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (other_id, _) = common::login_test_user(&app, &username, &password).await;
    attach_original(&state, other_id, id, "expense", None, Some("Original")).await;
    assert_eq!(list(&app, &token).await[0]["can_revert"], false);
    assert_eq!(
        revert(&app, &token, id).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        list(&app, &token).await[0]["description"],
        "Edited description"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn revert_handles_a_future_transfer_source_without_an_invalid_write() {
    let (state, app, user_id, token, id) = fixture(Arc::new(TestProvider::new(false, false))).await;
    attach_original(
        &state,
        user_id,
        id,
        "transfer",
        None,
        Some("Original transfer"),
    )
    .await;
    assert_eq!(list(&app, &token).await[0]["can_revert"], false);
    assert_eq!(
        revert(&app, &token, id).await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(list(&app, &token).await[0]["kind"], "Income");
}

#[tokio::test]
async fn embedding_failure_does_not_undo_revert_and_search_backfills_it() {
    let provider = Arc::new(TestProvider::new(true, false));
    let (state, app, user_id, token, id) = fixture(provider).await;
    attach_original(
        &state,
        user_id,
        id,
        "expense",
        None,
        Some("Original commute"),
    )
    .await;
    assert_eq!(revert(&app, &token, id).await.status(), StatusCode::OK);
    assert_eq!(
        list(&app, &token).await[0]["description"],
        "Original commute"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM transaction_embeddings WHERE transaction_id = $1")
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let response = app
        .clone()
        .oneshot(request(
            &token,
            "POST",
            "/api/transactions/search/semantic",
            serde_json::json!({"query": "Original commute"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let matches = json(response).await;
    assert_eq!(
        matches["transactions"][0]["transaction"]["description"],
        "Original commute"
    );
    assert_eq!(
        matches["transactions"][0]["transaction"]["can_revert"],
        false
    );
}

#[tokio::test]
async fn semantic_results_expose_revert_availability() {
    let (state, app, user_id, token, id) = fixture(Arc::new(TestProvider::new(false, false))).await;
    attach_original(
        &state,
        user_id,
        id,
        "expense",
        None,
        Some("Original commute"),
    )
    .await;
    let response = app
        .clone()
        .oneshot(request(
            &token,
            "POST",
            "/api/transactions/search/semantic",
            serde_json::json!({"query": "Edited description"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json(response).await["transactions"][0]["transaction"]["can_revert"],
        true
    );
}

#[tokio::test]
async fn delayed_revert_embedding_cannot_overwrite_a_newer_edit() {
    let provider = Arc::new(TestProvider::new(false, true));
    let (state, app, user_id, token, id) = fixture(provider.clone()).await;
    attach_original(
        &state,
        user_id,
        id,
        "expense",
        None,
        Some("Original commute"),
    )
    .await;
    let slow_app = app.clone();
    let slow_token = token.clone();
    let restore = tokio::spawn(async move { revert(&slow_app, &slow_token, id).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        provider.started.notified(),
    )
    .await
    .unwrap();
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), app.clone().oneshot(request(&token, "PUT", &format!("/api/transactions/{}", id), serde_json::json!({
        "amount": "80.00", "kind": "Income", "date": "2026-03-13", "category": "Newer", "description": "Newer edit"
    })))).await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    provider.release.notify_one();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), restore)
            .await
            .unwrap()
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let (text, vector): (String, Vector) = sqlx::query_as(
        "SELECT embedding_text, embedding FROM transaction_embeddings WHERE transaction_id = $1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert!(text.contains("Newer edit"));
    assert_eq!(vector.to_vec()[1], 1.0);
    assert_eq!(list(&app, &token).await[0]["can_revert"], true);
}
