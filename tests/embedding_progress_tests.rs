mod common;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use financetracker::embedding_worker::{EmbeddingBatchResult, process_embedding_batch};
use financetracker::embeddings::{EmbeddingProvider, store_transaction_embedding};
use financetracker::{AppState, build_app};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tower::util::ServiceExt;
use uuid::Uuid;

// Status reads must never call a provider. The same fake lets the final test run real batch writes.
#[derive(Default)]
struct RecordingEmbeddingProvider {
    calls: AtomicUsize,
    fail_next: AtomicBool,
}

impl RecordingEmbeddingProvider {
    fn fails_first_request() -> Self {
        Self {
            fail_next: AtomicBool::new(true),
            ..Default::default()
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EmbeddingProvider for RecordingEmbeddingProvider {
    async fn generate_embedding(
        &self,
        _http_client: &reqwest::Client,
        _openai_api_key: &str,
        _embedding_text: &str,
    ) -> Result<Vec<f32>, (StatusCode, String)> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err((
                StatusCode::BAD_GATEWAY,
                "Simulated embedding failure".to_string(),
            ));
        }
        Ok(test_embedding())
    }
}

fn test_embedding() -> Vec<f32> {
    let mut embedding = vec![0.0; 1536];
    embedding[0] = 1.0;
    embedding
}

async fn test_state(pool: PgPool, provider: Arc<RecordingEmbeddingProvider>) -> AppState {
    let mut state = common::setup_app_state_with_embedding_provider(provider).await;
    state.pool = pool;
    state
}

// Use normal registration and login so the endpoint must derive ownership from the authenticated JWT.
async fn register_user(state: &AppState) -> (Uuid, String) {
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    common::login_test_user(&app, &username, &password).await
}

// Save financial rows directly: fixture setup must not silently generate the embeddings being counted.
async fn save_transaction(
    state: &AppState,
    owner: Uuid,
    number: u128,
    status: &str,
    origin: &str,
) -> Uuid {
    let id = Uuid::from_u128(number);
    sqlx::query(
        "INSERT INTO transactions (id, user_id, amount, kind, category, date, description, origin, status, currency)
         VALUES ($1, $2, -12.50, 'expense', 'Food', '2026-10-17', $3, $4, $5, 'CAD')",
    )
    .bind(id).bind(owner).bind(format!("Indexing status transaction {number}"))
    .bind(origin).bind(status).execute(&state.pool).await.unwrap();
    id
}

// Explicit owner and revision arguments allow fixtures that look present but must not count as indexed.
async fn save_embedding(state: &AppState, id: Uuid, owner: Uuid, revision: i32) {
    store_transaction_embedding(
        state,
        id,
        owner,
        &format!(
            "kind: Expense\n category: Food\n description: Indexing status transaction {}",
            id.as_u128()
        ),
        test_embedding(),
        revision,
        "text-embedding-3-small",
    )
    .await
    .unwrap();
}

fn status_request(jwt: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method("GET")
        .uri("/api/transactions/indexing-status");
    if let Some(jwt) = jwt {
        request = request.header("Authorization", format!("Bearer {jwt}"));
    }
    request.body(Body::empty()).unwrap()
}

// Every successful read checks both the exact response contract and the absence of provider calls.
async fn assert_status(
    state: &AppState,
    provider: &RecordingEmbeddingProvider,
    jwt: &str,
    expected: Value,
) {
    let calls_before = provider.calls();
    let response = build_app(state.clone())
        .oneshot(status_request(Some(jwt)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), expected);
    assert_eq!(
        provider.calls(),
        calls_before,
        "Reading indexing status must not generate embeddings"
    );
}

async fn transaction_rows(state: &AppState) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(t) FROM transactions t ORDER BY id")
        .fetch_all(&state.pool)
        .await
        .unwrap()
}

async fn embedding_rows(state: &AppState) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(e) FROM transaction_embeddings e ORDER BY transaction_id")
        .fetch_all(&state.pool)
        .await
        .unwrap()
}

// Separate databases keep each status count independent of the other tests' unfinished transactions.
#[sqlx::test(migrations = "backend/migrations")]
async fn test_indexing_status_rejects_missing_invalid_and_expired_login(pool: PgPool) {
    let provider = Arc::new(RecordingEmbeddingProvider::default());
    let state = test_state(pool, provider.clone()).await;
    let (owner, _) = register_user(&state).await;
    save_transaction(&state, owner, 1, "posted", "imported").await;

    // An expired but correctly signed token must not expose the owner's count any more than a missing token can.
    let expired = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &json!({"sub": owner.to_string(), "exp": (Utc::now() - Duration::hours(2)).timestamp() as usize}),
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    ).unwrap();
    let app = build_app(state.clone());
    for jwt in [None, Some("invalid-jwt"), Some(expired.as_str())] {
        let response = app.clone().oneshot(status_request(jwt)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(provider.calls(), 0);
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_indexing_status_returns_zero_counts_for_empty_user(pool: PgPool) {
    let provider = Arc::new(RecordingEmbeddingProvider::default());
    let state = test_state(pool, provider.clone()).await;
    let (_, jwt) = register_user(&state).await;

    // An empty aggregate must return numeric zeros, rather than nulls or a missing response row.
    assert_status(
        &state,
        &provider,
        &jwt,
        json!({"total": 0, "indexed": 0, "remaining": 0}),
    )
    .await;
    assert!(transaction_rows(&state).await.is_empty());
    assert!(embedding_rows(&state).await.is_empty());
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_indexing_status_counts_only_owned_non_removed_transactions(pool: PgPool) {
    let provider = Arc::new(RecordingEmbeddingProvider::default());
    let state = test_state(pool, provider.clone()).await;
    let (owner, jwt) = register_user(&state).await;
    let (other_owner, other_jwt) = register_user(&state).await;

    // Both manual and imported rows count, and a bank-pending transaction is still eligible for indexing.
    save_transaction(&state, owner, 1, "posted", "manual").await;
    let imported = save_transaction(&state, owner, 2, "posted", "imported").await;
    let pending = save_transaction(&state, owner, 3, "pending", "imported").await;
    save_embedding(&state, imported, owner, 1).await;
    save_embedding(&state, pending, owner, 1).await;

    // Removed rows stay outside both sides of the progress count, whether or not they have an embedding.
    save_transaction(&state, owner, 4, "removed", "imported").await;
    let removed = save_transaction(&state, owner, 5, "removed", "imported").await;
    save_embedding(&state, removed, owner, 1).await;
    let foreign = save_transaction(&state, other_owner, 6, "posted", "imported").await;
    save_embedding(&state, foreign, other_owner, 1).await;
    save_transaction(&state, other_owner, 7, "posted", "manual").await;
    let before = (transaction_rows(&state).await, embedding_rows(&state).await);

    assert_status(
        &state,
        &provider,
        &jwt,
        json!({"total": 3, "indexed": 2, "remaining": 1}),
    )
    .await;
    assert_status(
        &state,
        &provider,
        &other_jwt,
        json!({"total": 2, "indexed": 1, "remaining": 1}),
    )
    .await;
    assert_eq!(
        (transaction_rows(&state).await, embedding_rows(&state).await),
        before
    );
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_indexing_status_keeps_missing_stale_and_wrong_owner_embeddings_outstanding(
    pool: PgPool,
) {
    let provider = Arc::new(RecordingEmbeddingProvider::default());
    let state = test_state(pool, provider.clone()).await;
    let (owner, jwt) = register_user(&state).await;
    let (other_owner, _) = register_user(&state).await;

    // Presence alone is insufficient: only the same transaction owner and exact current revision qualify.
    let current = save_transaction(&state, owner, 1, "posted", "imported").await;
    save_embedding(&state, current, owner, 1).await;
    save_transaction(&state, owner, 2, "posted", "manual").await;
    let stale = save_transaction(&state, owner, 3, "posted", "imported").await;
    save_embedding(&state, stale, owner, 0).await;
    let wrong_owner = save_transaction(&state, owner, 4, "posted", "imported").await;
    save_embedding(&state, wrong_owner, other_owner, 1).await;
    let wrong_revision = save_transaction(&state, owner, 5, "posted", "manual").await;
    save_embedding(&state, wrong_revision, owner, 2).await;
    let before = (transaction_rows(&state).await, embedding_rows(&state).await);

    // Reporting must leave these rows for the worker to repair, rather than performing that repair itself.
    assert_status(
        &state,
        &provider,
        &jwt,
        json!({"total": 5, "indexed": 1, "remaining": 4}),
    )
    .await;
    assert_eq!(
        (transaction_rows(&state).await, embedding_rows(&state).await),
        before
    );
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_indexing_status_advances_after_batches_and_keeps_failed_items_outstanding(
    pool: PgPool,
) {
    let provider = Arc::new(RecordingEmbeddingProvider::fails_first_request());
    let state = test_state(pool, provider.clone()).await;
    let (owner, jwt) = register_user(&state).await;
    save_transaction(&state, owner, 1, "posted", "imported").await;
    save_transaction(&state, owner, 2, "posted", "manual").await;
    let before = transaction_rows(&state).await;
    assert_status(
        &state,
        &provider,
        &jwt,
        json!({"total": 2, "indexed": 0, "remaining": 2}),
    )
    .await;

    // The actual bounded batch saves one embedding and fails the other; status reflects the committed result.
    assert_eq!(
        process_embedding_batch(&state, 10).await.unwrap(),
        EmbeddingBatchResult {
            indexed: 1,
            failed: 1,
            discarded: 0
        }
    );
    assert_eq!(provider.calls(), 2);
    assert_status(
        &state,
        &provider,
        &jwt,
        json!({"total": 2, "indexed": 1, "remaining": 1}),
    )
    .await;

    // A later successful retry reaches completion without any status request triggering extra provider work.
    assert_eq!(
        process_embedding_batch(&state, 10).await.unwrap(),
        EmbeddingBatchResult {
            indexed: 1,
            failed: 0,
            discarded: 0
        }
    );
    assert_eq!(provider.calls(), 3);
    assert_status(
        &state,
        &provider,
        &jwt,
        json!({"total": 2, "indexed": 2, "remaining": 0}),
    )
    .await;
    assert_eq!(transaction_rows(&state).await, before);
}
