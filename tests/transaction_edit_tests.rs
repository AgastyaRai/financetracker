// behavioral specification for editing transactions and refreshing derived embeddings
mod common;

use async_trait::async_trait;
use financetracker::embeddings::{
    EmbeddingProvider,
    TransactionEmbedding,
    store_transaction_embedding_if_current,
};
use financetracker::{AppState, build_app};
use http_body_util::BodyExt;
use pgvector::Vector;
use sqlx::Row;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;
use tower::util::ServiceExt;

struct RecordingEmbeddingProvider {
    call_count: AtomicUsize,
    fail_on_call: Option<usize>,
}

impl RecordingEmbeddingProvider {
    fn succeeds() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            fail_on_call: None,
        }
    }

    fn fails_on_call(call: usize) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            fail_on_call: Some(call),
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EmbeddingProvider for RecordingEmbeddingProvider {
    async fn generate_embedding(
        &self,
        _http_client: &reqwest::Client,
        _openai_api_key: &str,
        embedding_text: &str,
    ) -> Result<Vec<f32>, (axum::http::StatusCode, String)> {
        let call = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

        if self.fail_on_call == Some(call) {
            return Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!("simulated embedding failure on call {}", call),
            ));
        }

        Ok(embedding_for_text(embedding_text))
    }
}

struct BlockingEmbeddingProvider {
    call_count: AtomicUsize,
    first_update_started: Notify,
    release_first_update: Notify,
}

impl BlockingEmbeddingProvider {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            first_update_started: Notify::new(),
            release_first_update: Notify::new(),
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EmbeddingProvider for BlockingEmbeddingProvider {
    async fn generate_embedding(
        &self,
        _http_client: &reqwest::Client,
        _openai_api_key: &str,
        embedding_text: &str,
    ) -> Result<Vec<f32>, (axum::http::StatusCode, String)> {
        let call = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

        // call 1 creates the original embedding; call 2 belongs to the first update
        if call == 2 {
            self.first_update_started.notify_one();
            self.release_first_update.notified().await;
        }

        Ok(embedding_for_text(embedding_text))
    }
}

fn embedding_for_text(embedding_text: &str) -> Vec<f32> {
    let embedding_text = embedding_text.to_ascii_lowercase();
    let matching_dimension = if embedding_text.contains("second concurrent") {
        2
    } else if embedding_text.contains("first concurrent") {
        1
    } else if embedding_text.contains("updated") {
        3
    } else {
        0
    };

    let mut embedding = vec![0.0; 1536];
    embedding[matching_dimension] = 1.0;
    embedding
}

fn transaction_body(
    amount: f64,
    date: &str,
    category: &str,
    description: &str,
) -> serde_json::Value {
    serde_json::json!({
        "amount": amount,
        "kind": "Expense",
        "date": date,
        "category": category,
        "description": description
    })
}

fn update_request(
    access_token: &str,
    transaction_id: uuid::Uuid,
    body: serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("PUT")
        .uri(format!("/api/transactions/{}", transaction_id))
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

#[derive(serde::Deserialize)]
struct TransactionIdentity {
    id: uuid::Uuid,
    description: Option<String>,
}

async fn setup_user(
    embedding_provider: Arc<dyn EmbeddingProvider>,
) -> (AppState, axum::Router, uuid::Uuid, String) {
    let state = common::setup_app_state_with_embedding_provider(embedding_provider).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    (state, app, user_id, access_token)
}

async fn add_transaction_and_get_id(
    app: &axum::Router,
    access_token: &str,
    description: &str,
) -> uuid::Uuid {
    common::add_transaction(
        app,
        access_token,
        transaction_body(50.00, "2026-03-01", "Transportation", description),
    )
    .await;

    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/api/transactions")
        .header("Authorization", format!("Bearer {}", access_token))
        .body(axum::body::Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let transactions: Vec<TransactionIdentity> = serde_json::from_slice(&body).unwrap();

    transactions
        .into_iter()
        .find(|transaction| transaction.description.as_deref() == Some(description))
        .expect("created transaction was not returned by the API")
        .id
}

async fn stored_embedding(
    state: &AppState,
    transaction_id: uuid::Uuid,
) -> Option<(String, Vec<f32>)> {
    let row = sqlx::query(
        "SELECT embedding_text, embedding
         FROM transaction_embeddings
         WHERE transaction_id = $1",
    )
    .bind(transaction_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap();

    row.map(|row| {
        let embedding_text: String = row.get("embedding_text");
        let embedding: Vector = row.get("embedding");
        (embedding_text, embedding.to_vec())
    })
}

// guarded storage writes only when the transaction still matches the semantic fields that were embedded
#[tokio::test]
async fn test_guarded_embedding_store_rejects_stale_transaction_fields() {
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, user_id, access_token) = setup_user(provider).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    let current_embedding_text =
        "kind: Expense\n category: Transportation\n description: Original commute";
    let current_embedding = embedding_for_text(current_embedding_text);
    let current_transaction_embedding = TransactionEmbedding::generate(
        &state,
        "expense",
        Some("Transportation"),
        Some("Original commute"),
    )
    .await
    .unwrap();
    let stored = store_transaction_embedding_if_current(
        &state,
        transaction_id,
        user_id,
        current_transaction_embedding,
    )
    .await
    .unwrap();

    assert!(stored);
    assert_eq!(
        stored_embedding(&state, transaction_id).await.unwrap(),
        (current_embedding_text.to_string(), current_embedding.clone())
    );

    let stale_transaction_embedding = TransactionEmbedding::generate(
        &state,
        "expense",
        Some("Travel"),
        Some("Stale provider response"),
    )
    .await
    .unwrap();
    let stale_store = store_transaction_embedding_if_current(
        &state,
        transaction_id,
        user_id,
        stale_transaction_embedding,
    )
    .await
    .unwrap();

    assert!(!stale_store);
    assert_eq!(
        stored_embedding(&state, transaction_id).await.unwrap(),
        (current_embedding_text.to_string(), current_embedding)
    );
}

// updating fields that are not included in embedding text should preserve the existing embedding
#[tokio::test]
async fn test_update_amount_and_date_does_not_regenerate_embedding() {
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;
    let original_embedding = stored_embedding(&state, transaction_id).await.unwrap();

    let request = update_request(
        &access_token,
        transaction_id,
        transaction_body(75.00, "2026-03-02", "Transportation", "Original commute"),
    );
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let row = sqlx::query(
        "SELECT amount, date, description FROM transactions WHERE id = $1",
    )
    .bind(transaction_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let amount: rust_decimal::Decimal = row.get("amount");
    let date: chrono::NaiveDate = row.get("date");
    let description: Option<String> = row.get("description");

    assert_eq!(amount, rust_decimal::Decimal::new(7500, 2));
    assert_eq!(date, chrono::NaiveDate::from_ymd_opt(2026, 3, 2).unwrap());
    assert_eq!(description.as_deref(), Some("Original commute"));
    assert_eq!(stored_embedding(&state, transaction_id).await.unwrap(), original_embedding);
    assert_eq!(provider.call_count(), 1);
}

// changing kind, category, or description should replace the canonical text and vector
#[tokio::test]
async fn test_update_semantic_fields_replaces_embedding() {
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    let request = update_request(
        &access_token,
        transaction_id,
        transaction_body(50.00, "2026-03-01", "Travel", "Updated train commute"),
    );
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let (embedding_text, embedding) = stored_embedding(&state, transaction_id).await.unwrap();
    assert_eq!(
        embedding_text,
        "kind: Expense\n category: Travel\n description: Updated train commute"
    );
    assert_eq!(embedding, embedding_for_text("Updated train commute"));
    assert_eq!(provider.call_count(), 2);
}

// an external provider failure must not roll back the financial update or leave stale search data
#[tokio::test]
async fn test_update_provider_failure_preserves_transaction_and_removes_stale_embedding() {
    let provider = Arc::new(RecordingEmbeddingProvider::fails_on_call(2));
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    let request = update_request(
        &access_token,
        transaction_id,
        transaction_body(50.00, "2026-03-01", "Travel", "Updated train commute"),
    );
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let row = sqlx::query("SELECT category, description FROM transactions WHERE id = $1")
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let category: Option<String> = row.get("category");
    let description: Option<String> = row.get("description");

    assert_eq!(category.as_deref(), Some("Travel"));
    assert_eq!(description.as_deref(), Some("Updated train commute"));
    assert!(stored_embedding(&state, transaction_id).await.is_none());
    assert_eq!(provider.call_count(), 2);
}

// semantic search should recover an embedding that was left missing after an update failure
#[tokio::test]
async fn test_search_backfills_embedding_after_update_provider_failure() {
    let provider = Arc::new(RecordingEmbeddingProvider::fails_on_call(2));
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    let update = update_request(
        &access_token,
        transaction_id,
        transaction_body(50.00, "2026-03-01", "Travel", "Updated train commute"),
    );
    let update_response = app.clone().oneshot(update).await.unwrap();
    assert_eq!(update_response.status(), axum::http::StatusCode::OK);
    assert!(stored_embedding(&state, transaction_id).await.is_none());

    let search_body = serde_json::json!({
        "query": "updated commute",
        "limit": 5
    });
    let search_request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/transactions/search/semantic")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(search_body.to_string()))
        .unwrap();
    let search_response = app.oneshot(search_request).await.unwrap();

    assert_eq!(search_response.status(), axum::http::StatusCode::OK);
    let body = search_response.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let transaction_id_string = transaction_id.to_string();
    let returned_updated_transaction = result["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| {
            result["transaction"]["id"].as_str() == Some(transaction_id_string.as_str())
                && result["transaction"]["description"].as_str() == Some("Updated train commute")
        });

    assert!(returned_updated_transaction);
    assert!(stored_embedding(&state, transaction_id).await.is_some());
    assert_eq!(provider.call_count(), 4);
}

// ownership is checked in the update query so one user cannot modify another user's transaction
#[tokio::test]
async fn test_user_cannot_update_another_users_transaction() {
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, _first_user_id, first_access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &first_access_token,
        "Original commute",
    )
    .await;

    let (second_username, second_password) = common::create_and_register_test_user(&app).await;
    let (_second_user_id, second_access_token) =
        common::login_test_user(&app, &second_username, &second_password).await;
    let request = update_request(
        &second_access_token,
        transaction_id,
        transaction_body(50.00, "2026-03-01", "Travel", "Unauthorized update"),
    );
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);

    let row = sqlx::query("SELECT description FROM transactions WHERE id = $1")
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let description: Option<String> = row.get("description");

    assert_eq!(description.as_deref(), Some("Original commute"));
    assert!(stored_embedding(&state, transaction_id).await.is_some());
    assert_eq!(provider.call_count(), 1);
}

// a slow older embedding response must not overwrite the result of a newer update
#[tokio::test]
async fn test_delayed_update_cannot_overwrite_newer_embedding() {
    let provider = Arc::new(BlockingEmbeddingProvider::new());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    let first_request = update_request(
        &access_token,
        transaction_id,
        transaction_body(50.00, "2026-03-01", "Travel", "First concurrent update"),
    );
    let first_app = app.clone();
    let first_update = tokio::spawn(async move { first_app.oneshot(first_request).await.unwrap() });

    tokio::time::timeout(Duration::from_secs(2), provider.first_update_started.notified())
        .await
        .expect("first update did not reach embedding generation");

    let second_request = update_request(
        &access_token,
        transaction_id,
        transaction_body(50.00, "2026-03-01", "Travel", "Second concurrent update"),
    );
    let second_response = tokio::time::timeout(
        Duration::from_secs(5),
        app.oneshot(second_request),
    )
    .await
    .expect("second update was blocked by the first provider call")
    .unwrap();
    assert_eq!(second_response.status(), axum::http::StatusCode::OK);

    provider.release_first_update.notify_one();
    let first_response = tokio::time::timeout(Duration::from_secs(5), first_update)
        .await
        .expect("first update did not finish after provider release")
        .unwrap();
    assert_eq!(first_response.status(), axum::http::StatusCode::OK);

    let row = sqlx::query("SELECT description FROM transactions WHERE id = $1")
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let description: Option<String> = row.get("description");
    let (embedding_text, embedding) = stored_embedding(&state, transaction_id).await.unwrap();

    assert_eq!(description.as_deref(), Some("Second concurrent update"));
    assert_eq!(
        embedding_text,
        "kind: Expense\n category: Travel\n description: Second concurrent update"
    );
    assert_eq!(embedding, embedding_for_text("Second concurrent update"));
    assert_eq!(provider.call_count(), 3);
}

// transaction creation rejects non-positive amounts before writing data or calling the embedding provider
#[tokio::test]
async fn test_create_rejects_non_positive_amount() {
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, user_id, access_token) = setup_user(provider.clone()).await;
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/transactions")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            transaction_body(0.00, "2026-03-01", "Transportation", "Invalid amount")
                .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);

    let transaction_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transactions WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();

    assert_eq!(transaction_count, 0);
    assert_eq!(provider.call_count(), 0);
}

// a rejected update preserves the existing transaction and its embedding
#[tokio::test]
async fn test_update_rejects_non_positive_amount() {
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;
    let original_embedding = stored_embedding(&state, transaction_id).await.unwrap();
    let request = update_request(
        &access_token,
        transaction_id,
        transaction_body(-10.00, "2026-03-02", "Travel", "Invalid update"),
    );

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);

    let row = sqlx::query("SELECT amount, date, category, description FROM transactions WHERE id = $1")
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let amount: rust_decimal::Decimal = row.get("amount");
    let date: chrono::NaiveDate = row.get("date");
    let category: Option<String> = row.get("category");
    let description: Option<String> = row.get("description");

    assert_eq!(amount, rust_decimal::Decimal::new(5000, 2));
    assert_eq!(date, chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
    assert_eq!(category.as_deref(), Some("Transportation"));
    assert_eq!(description.as_deref(), Some("Original commute"));
    assert_eq!(stored_embedding(&state, transaction_id).await.unwrap(), original_embedding);
    assert_eq!(provider.call_count(), 1);
}

// the database constraint protects transaction amounts even when a write bypasses the HTTP handlers
#[tokio::test]
async fn test_database_rejects_non_positive_transaction_amount() {
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, _app, user_id, _access_token) = setup_user(provider).await;

    let result = sqlx::query(
        "INSERT INTO transactions (user_id, amount, kind, category, date, description)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(user_id)
    .bind(rust_decimal::Decimal::new(-100, 2))
    .bind("expense")
    .bind(Some("Transportation"))
    .bind(chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap())
    .bind(Some("Invalid direct insert"))
    .execute(&state.pool)
    .await;

    assert!(result.is_err());
}
