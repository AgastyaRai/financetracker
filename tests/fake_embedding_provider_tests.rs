mod common;

use async_trait::async_trait;
use financetracker::embeddings::{EmbeddingProvider, store_transaction_embedding};
use financetracker::build_app;
use http_body_util::BodyExt;
use pgvector::Vector;
use sqlx::Row;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::util::ServiceExt;
use uuid::Uuid;

#[derive(Clone)]
// choose the vector or error that the fake provider should return
enum FakeEmbeddingOutcome {
    Success(Vec<f32>),
    Failure(String),
}

// fake provider that returns the chosen outcome and counts requests without calling the real API
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
        // record the request so tests can check whether the app called the provider
        self.call_count.fetch_add(1, Ordering::SeqCst);

        // return the vector or simulated error selected when this provider was created
        match &self.outcome {
            FakeEmbeddingOutcome::Success(embedding) => Ok(embedding.clone()),
            FakeEmbeddingOutcome::Failure(error) => Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                error.clone(),
            )),
        }
    }
}

// build a fixed nonzero vector with the 1536 dimensions expected by the database
fn test_embedding() -> Vec<f32> {
    let mut embedding = vec![0.0; 1536];
    embedding[0] = 1.0;
    embedding
}

// see if creating a transaction stores the fake provider's vector with its revision and model
#[tokio::test]
async fn test_fake_provider_stores_embedding() {
    // use a successful fake provider, build the router, and register + log in a user
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    // create an expense through the API so the normal handler generates and stores its embedding
    let transaction = serde_json::json!({
        "amount": -25.00,
        "kind": "Expense",
        "date": "2026-02-01",
        "category": "Food",
        "description": "Fake provider success transaction"
    });

    common::add_transaction(&app, &access_token, transaction).await;

    // read the transaction and its embedding together to compare their revisions and stored vector
    let row = sqlx::query(
        "SELECT e.embedding,
                t.search_revision AS transaction_search_revision,
                e.search_revision AS embedding_search_revision,
                e.embedding_model
         FROM transactions t
         JOIN transaction_embeddings e ON e.transaction_id = t.id
         WHERE t.user_id = $1 AND t.description = $2",
    )
    .bind(user_id)
    .bind("Fake provider success transaction")
    .fetch_one(&state.pool)
    .await
    .unwrap();

    // extract the stored vector, revisions, and model from the database row
    let stored_embedding: Vector = row.get("embedding");
    let transaction_search_revision: i32 = row.get("transaction_search_revision");
    let embedding_search_revision: i32 = row.get("embedding_search_revision");
    let embedding_model: String = row.get("embedding_model");
    // the vector should match the fake response, both revisions should be 1, and creation should make one request
    assert_eq!(stored_embedding.to_vec(), test_embedding());
    assert_eq!(transaction_search_revision, 1);
    assert_eq!(embedding_search_revision, 1);
    assert_eq!(embedding_model, "text-embedding-3-small");
    assert_eq!(provider.call_count(), 1);
}

// see if the direct embedding writer stores the supplied revision and model
#[tokio::test]
async fn test_raw_embedding_writer_stores_revision_and_model() {
    // set up app state and a registered user for the database rows in this test
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, _access_token) = common::login_test_user(&app, &username, &password).await;

    // insert a transaction directly so it starts without an embedding generated by the create handler
    let transaction_row = sqlx::query(
        "INSERT INTO transactions (user_id, amount, kind, category, date, description)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id",
    )
    .bind(user_id)
    .bind(rust_decimal::Decimal::new(-3000, 2))
    .bind("expense")
    .bind(Some("Food".to_string()))
    .bind(chrono::NaiveDate::from_ymd_opt(2026, 2, 4).unwrap())
    .bind(Some("Raw embedding writer transaction".to_string()))
    .fetch_one(&state.pool)
    .await
    .unwrap();

    let transaction_id: uuid::Uuid = transaction_row.get("id");
    // call the writer directly with a known vector, revision, and model
    store_transaction_embedding(
        &state,
        transaction_id,
        user_id,
        "kind: Expense\n category: Food\n description: Raw embedding writer transaction",
        test_embedding(),
        1,
        "text-embedding-3-small",
    )
    .await
    .unwrap();

    // read the stored metadata to check that the writer included the required database fields
    let embedding_row = sqlx::query(
        "SELECT search_revision, embedding_model
         FROM transaction_embeddings
         WHERE transaction_id = $1",
    )
    .bind(transaction_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();

    let search_revision: i32 = embedding_row.get("search_revision");
    let embedding_model: String = embedding_row.get("embedding_model");
    // the stored revision and model should match the values passed to the writer
    assert_eq!(search_revision, 1);
    assert_eq!(embedding_model, "text-embedding-3-small");
}

// see if creating a transaction still succeeds when the embedding provider fails
#[tokio::test]
async fn test_transaction_creation_succeeds_when_fake_provider_fails() {
    // use a provider that always returns a simulated outage, then register + log in a user
    let provider = Arc::new(FakeEmbeddingProvider::fails_with("simulated embedding outage"));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    // create an expense through the API; the helper checks that the request still returns Created
    let transaction = serde_json::json!({
        "amount": -40.00,
        "kind": "Expense",
        "date": "2026-02-02",
        "category": "Utilities",
        "description": "Fake provider failure transaction"
    });

    common::add_transaction(&app, &access_token, transaction).await;

    // find the saved transaction to prove the provider failure did not undo the financial write
    let transaction_row = sqlx::query(
        "SELECT id FROM transactions WHERE user_id = $1 AND description = $2",
    )
    .bind(user_id)
    .bind("Fake provider failure transaction")
    .fetch_one(&state.pool)
    .await
    .unwrap();

    let transaction_id: uuid::Uuid = transaction_row.get("id");
    // check whether an embedding was stored for the transaction despite the failed provider request
    let embedding_row = sqlx::query(
        "SELECT transaction_id FROM transaction_embeddings WHERE transaction_id = $1",
    )
    .bind(transaction_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap();

    // the transaction should exist without an embedding, and the provider should have been called once
    assert!(embedding_row.is_none());
    assert_eq!(provider.call_count(), 1);
}

// see if searching generates and stores an embedding for a transaction that has none
#[tokio::test]
async fn test_fake_provider_backfills_missing_embedding() {
    // use a successful fake provider and set up the user who will perform the search
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    // insert the transaction directly to leave it without an embedding at the start of the test
    let transaction_row = sqlx::query(
        "INSERT INTO transactions (user_id, amount, kind, category, date, description)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id",
    )
    .bind(user_id)
    .bind(rust_decimal::Decimal::new(-5500, 2))
    .bind("expense")
    .bind(Some("Transportation".to_string()))
    .bind(chrono::NaiveDate::from_ymd_opt(2026, 2, 3).unwrap())
    .bind(Some("Fake provider backfill transaction".to_string()))
    .fetch_one(&state.pool)
    .await
    .unwrap();

    let transaction_id: uuid::Uuid = transaction_row.get("id");
    // search for transportation so the handler has to fill in the missing embedding before searching
    let search_body = serde_json::json!({
        "query": "transportation",
        "limit": 5
    });

    // build the authenticated search request and send it through the router
    let search_request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/transactions/search/semantic")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(search_body.to_string()))
        .unwrap();

    let search_response = app.oneshot(search_request).await.unwrap();
    assert_eq!(search_response.status(), axum::http::StatusCode::OK);

    // read the newly stored embedding alongside the transaction's current revision
    let embedding_row = sqlx::query(
        "SELECT e.embedding,
                t.search_revision AS transaction_search_revision,
                e.search_revision AS embedding_search_revision,
                e.embedding_model
         FROM transactions t
         JOIN transaction_embeddings e ON e.transaction_id = t.id
         WHERE t.id = $1",
    )
    .bind(transaction_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();

    // extract the vector, revisions, and model from the stored result
    let stored_embedding: Vector = embedding_row.get("embedding");
    let transaction_search_revision: i32 = embedding_row.get("transaction_search_revision");
    let embedding_search_revision: i32 = embedding_row.get("embedding_search_revision");
    let embedding_model: String = embedding_row.get("embedding_model");
    // the replacement should use the fake vector and revision 1 with the expected model
    assert_eq!(stored_embedding.to_vec(), test_embedding());
    assert_eq!(transaction_search_revision, 1);
    assert_eq!(embedding_search_revision, 1);
    assert_eq!(embedding_model, "text-embedding-3-small");
    // expect one call for the missing transaction embedding and one for the search-query embedding
    assert_eq!(provider.call_count(), 2);
}

// see if a search containing only spaces is rejected before calling the embedding provider
#[tokio::test]
async fn test_semantic_search_rejects_blank_query_without_calling_provider() {
    // set up a user with a fake provider that counts any embedding requests
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state);
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (_user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    // use spaces as the query to check that validation trims it before checking for empty text
    let search_body = serde_json::json!({
        "query": "   ",
        "limit": 5
    });

    // send the invalid query as an authenticated user
    let search_request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/transactions/search/semantic")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(search_body.to_string()))
        .unwrap();

    let search_response = app.oneshot(search_request).await.unwrap();
    // the API should reject the query without making any provider request
    assert_eq!(search_response.status(), axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(provider.call_count(), 0);
}

// see if a search query longer than 500 characters is rejected before calling the provider
#[tokio::test]
async fn test_semantic_search_rejects_long_query_without_calling_provider() {
    // set up a user with a fake provider that counts any embedding requests
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state);
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (_user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    // make the query one character over the limit to check the validation boundary
    let search_body = serde_json::json!({
        "query": "a".repeat(501),
        "limit": 5
    });

    // send the oversized query as an authenticated user
    let search_request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/transactions/search/semantic")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(search_body.to_string()))
        .unwrap();

    let search_response = app.oneshot(search_request).await.unwrap();
    // the API should reject the query without making any provider request
    assert_eq!(search_response.status(), axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(provider.call_count(), 0);
}

#[derive(serde::Deserialize)]
struct ReturnedSearchTransaction {
    transaction: financetracker::Transaction,
}

#[derive(serde::Deserialize)]
struct ReturnedSearchResult {
    transactions: Vec<ReturnedSearchTransaction>,
}

#[derive(sqlx::FromRow)]
struct StoredSearchTransaction {
    status: String,
    embedding: Option<Vector>,
}

#[tokio::test]
async fn test_semantic_search_excludes_removed_transactions_before_limit() {
    // use a fixed query vector and register a separate user for the three search candidates
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    // store current embeddings for every row so this test exercises result filtering without backfill
    // all vectors qualify, but removed is the strongest match, followed by posted and then pending
    let mut transaction_ids = Vec::new();
    for (status, matching_component, other_component) in [
        ("pending", 0.8_f32, 0.2_f32),
        ("posted", 0.9_f32, 0.1_f32),
        ("removed", 1.0_f32, 0.0_f32),
    ] {
        let description = format!("{} imported transaction", status);
        let transaction_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO transactions (user_id, amount, kind, category, date, description, origin, status)
             VALUES ($1, -10.00, 'expense', 'Food', '2026-03-05', $2, 'imported', $3)
             RETURNING id",
        )
        .bind(user_id)
        .bind(&description)
        .bind(status)
        .fetch_one(&state.pool)
        .await
        .unwrap();

        let mut embedding = test_embedding();
        embedding[0] = matching_component;
        embedding[1] = other_component;
        let embedding_text = format!(
            "kind: Expense\n category: Food\n description: {}",
            description
        );
        store_transaction_embedding(
            &state,
            transaction_id,
            user_id,
            &embedding_text,
            embedding,
            1,
            "text-embedding-3-small",
        )
        .await
        .unwrap();

        transaction_ids.push(transaction_id);
    }
    let [pending_id, posted_id, removed_id]: [Uuid; 3] = transaction_ids.try_into().unwrap();

    // limit the search to two results so the removed row would displace pending unless filtering happens in SQL
    let search_body = serde_json::json!({
        "query": "food",
        "limit": 2,
        "summary": false
    });
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/transactions/search/semantic")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(search_body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let result: ReturnedSearchResult = serde_json::from_slice(&body).unwrap();

    // excluding a removed row from search must preserve its status and existing embedding
    let removed = sqlx::query_as::<_, StoredSearchTransaction>(
        "SELECT t.status, e.embedding
         FROM transactions t
         LEFT JOIN transaction_embeddings e ON e.transaction_id = t.id
         WHERE t.id = $1",
    )
    .bind(removed_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(removed.status, "removed");
    assert_eq!(removed.embedding.unwrap().to_vec(), test_embedding());
    assert_eq!(provider.call_count(), 1);

    // both eligible rows must fill the two available slots, regardless of their returned order
    let mut returned_ids: Vec<Uuid> = result
        .transactions
        .iter()
        .map(|result| result.transaction.id)
        .collect();
    let mut expected_ids = vec![pending_id, posted_id];
    returned_ids.sort_unstable();
    expected_ids.sort_unstable();
    assert_eq!(
        returned_ids, expected_ids,
        "removed transactions must be excluded before applying the search limit"
    );
}

#[tokio::test]
async fn test_semantic_search_skips_removed_transactions_during_backfill() {
    // count provider requests while searching a user who has two rows without embeddings
    let provider = Arc::new(FakeEmbeddingProvider::succeeds_with(test_embedding()));
    let state = common::setup_app_state_with_embedding_provider(provider.clone()).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    // insert posted and removed imports directly so neither starts with an embedding
    let mut transaction_ids = Vec::new();
    for status in ["posted", "removed"] {
        let transaction_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO transactions (user_id, amount, kind, category, date, description, origin, status)
             VALUES ($1, -10.00, 'expense', 'Food', '2026-03-05', 'Imported lunch', 'imported', $2)
             RETURNING id",
        )
        .bind(user_id)
        .bind(status)
        .fetch_one(&state.pool)
        .await
        .unwrap();

        transaction_ids.push(transaction_id);
    }
    let [posted_id, removed_id]: [Uuid; 2] = transaction_ids.try_into().unwrap();

    // searching should backfill the posted transaction, skip the removed one, and embed the query
    let search_body = serde_json::json!({
        "query": "food",
        "limit": 5,
        "summary": false
    });
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/transactions/search/semantic")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(search_body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let result: ReturnedSearchResult = serde_json::from_slice(&body).unwrap();

    // the posted row must gain the fake vector while the removed row remains stored without an embedding
    let posted = sqlx::query_as::<_, StoredSearchTransaction>(
        "SELECT t.status, e.embedding
         FROM transactions t
         LEFT JOIN transaction_embeddings e ON e.transaction_id = t.id
         WHERE t.id = $1",
    )
    .bind(posted_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let removed = sqlx::query_as::<_, StoredSearchTransaction>(
        "SELECT t.status, e.embedding
         FROM transactions t
         LEFT JOIN transaction_embeddings e ON e.transaction_id = t.id
         WHERE t.id = $1",
    )
    .bind(removed_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();

    assert_eq!(posted.status, "posted");
    assert_eq!(posted.embedding.unwrap().to_vec(), test_embedding());
    assert_eq!(removed.status, "removed");
    assert_eq!(
        provider.call_count(),
        2,
        "only the posted transaction and the search query should call the provider"
    );
    assert!(removed.embedding.is_none());

    // the posted transaction should also be the only result returned by this search
    let returned_ids: Vec<Uuid> = result
        .transactions
        .iter()
        .map(|result| result.transaction.id)
        .collect();
    assert_eq!(returned_ids, vec![posted_id]);
}
