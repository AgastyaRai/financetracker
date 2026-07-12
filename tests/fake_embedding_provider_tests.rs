mod common;

use async_trait::async_trait;
use financetracker::embeddings::EmbeddingProvider;
use financetracker::build_app;
use pgvector::Vector;
use sqlx::Row;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::util::ServiceExt;

#[derive(Clone)]
enum FakeEmbeddingOutcome {
    Success(Vec<f32>),
    Failure(String),
}

struct FakeEmbeddingProvider {
    outcome: FakeEmbeddingOutcome,
    call_count: AtomicUsize,
}

impl FakeEmbeddingProvider {
    fn succeeds_with(embedding: Vec<f32>) -> Self {
        Self {
            outcome: FakeEmbeddingOutcome::Success(embedding),
            call_count: AtomicUsize::new(0),
        }
    }

    fn fails_with(error: &str) -> Self {
        Self {
            outcome: FakeEmbeddingOutcome::Failure(error.to_string()),
            call_count: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EmbeddingProvider for FakeEmbeddingProvider {
    async fn generate_embedding(
        &self,
        _http_client: &reqwest::Client,
        _openai_api_key: &str,
        _embedding_text: &str,
    ) -> Result<Vec<f32>, (axum::http::StatusCode, String)> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        match &self.outcome {
            FakeEmbeddingOutcome::Success(embedding) => Ok(embedding.clone()),
            FakeEmbeddingOutcome::Failure(error) => Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                error.clone(),
            )),
        }
    }
}

fn test_embedding() -> Vec<f32> {
    let mut embedding = vec![0.0; 1536];
    embedding[0] = 1.0;
    embedding
}

#[tokio::test]
async fn test_fake_provider_stores_embedding() {
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    let transaction = serde_json::json!({
        "amount": 25.00,
        "kind": "Expense",
        "date": "2026-02-01",
        "category": "Food",
        "description": "Fake provider success transaction"
    });

    common::add_transaction(&app, &access_token, transaction).await;

    let row = sqlx::query(
        "SELECT e.embedding
         FROM transactions t
         JOIN transaction_embeddings e ON e.transaction_id = t.id
         WHERE t.user_id = $1 AND t.description = $2",
    )
    .bind(user_id)
    .bind("Fake provider success transaction")
    .fetch_one(&state.pool)
    .await
    .unwrap();

    let stored_embedding: Vector = row.get("embedding");
    assert_eq!(stored_embedding.to_vec(), test_embedding());
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn test_transaction_creation_succeeds_when_fake_provider_fails() {
    let provider = Arc::new(FakeEmbeddingProvider::fails_with("simulated embedding outage"));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    let transaction = serde_json::json!({
        "amount": 40.00,
        "kind": "Expense",
        "date": "2026-02-02",
        "category": "Utilities",
        "description": "Fake provider failure transaction"
    });

    common::add_transaction(&app, &access_token, transaction).await;

    let transaction_row = sqlx::query(
        "SELECT id FROM transactions WHERE user_id = $1 AND description = $2",
    )
    .bind(user_id)
    .bind("Fake provider failure transaction")
    .fetch_one(&state.pool)
    .await
    .unwrap();

    let transaction_id: uuid::Uuid = transaction_row.get("id");
    let embedding_row = sqlx::query(
        "SELECT transaction_id FROM transaction_embeddings WHERE transaction_id = $1",
    )
    .bind(transaction_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap();

    assert!(embedding_row.is_none());
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn test_fake_provider_backfills_missing_embedding() {
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    let transaction_row = sqlx::query(
        "INSERT INTO transactions (user_id, amount, kind, category, date, description)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id",
    )
    .bind(user_id)
    .bind(rust_decimal::Decimal::new(5500, 2))
    .bind("expense")
    .bind(Some("Transportation".to_string()))
    .bind(chrono::NaiveDate::from_ymd_opt(2026, 2, 3).unwrap())
    .bind(Some("Fake provider backfill transaction".to_string()))
    .fetch_one(&state.pool)
    .await
    .unwrap();

    let transaction_id: uuid::Uuid = transaction_row.get("id");
    let search_body = serde_json::json!({
        "query": "transportation",
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

    let embedding_row = sqlx::query(
        "SELECT embedding FROM transaction_embeddings WHERE transaction_id = $1",
    )
    .bind(transaction_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();

    let stored_embedding: Vector = embedding_row.get("embedding");
    assert_eq!(stored_embedding.to_vec(), test_embedding());
    assert_eq!(provider.call_count(), 2);
}

#[tokio::test]
async fn test_semantic_search_rejects_blank_query_without_calling_provider() {
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state);
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (_user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    let search_body = serde_json::json!({
        "query": "   ",
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
    assert_eq!(search_response.status(), axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(provider.call_count(), 0);
}

#[tokio::test]
async fn test_semantic_search_rejects_long_query_without_calling_provider() {
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state);
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (_user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    let search_body = serde_json::json!({
        "query": "a".repeat(501),
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
    assert_eq!(search_response.status(), axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(provider.call_count(), 0);
}
