// tests for editing transactions and keeping their search embeddings up to date
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

const EXPECTED_EMBEDDING_MODEL: &str = "text-embedding-3-small";

// fake embedding provider that counts requests and can fail on a chosen request
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

    // choose which request should fail so creation can succeed before an update fails
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
        // count this request so tests can check when another embedding was generated
        let call = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

        // return a simulated provider error only on the request selected by the test
        if self.fail_on_call == Some(call) {
            return Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!("simulated embedding failure on call {}", call),
            ));
        }

        // return a predictable vector based on the text without calling the real API
        Ok(embedding_for_text(embedding_text))
    }
}

// fake provider that pauses the first edit so a second edit can finish before it
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

        // the first request creates the transaction embedding; the second comes from the first edit
        // let the test know the edit reached the provider, then wait until the test releases it
        if call == 2 {
            self.first_update_started.notify_one();
            self.release_first_update.notified().await;
        }

        Ok(embedding_for_text(embedding_text))
    }
}

// fake provider for changing a description, changing it again, and then returning to the first value
struct AbaBlockingEmbeddingProvider {
    call_count: AtomicUsize,
    first_update_started: Notify,
    release_first_update: Notify,
}

impl AbaBlockingEmbeddingProvider {
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
impl EmbeddingProvider for AbaBlockingEmbeddingProvider {
    async fn generate_embedding(
        &self,
        _http_client: &reqwest::Client,
        _openai_api_key: &str,
        _embedding_text: &str,
    ) -> Result<Vec<f32>, (axum::http::StatusCode, String)> {
        let call = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

        // pause the first edit while two later edits change the description and then change it back
        // the delayed result has the same text as the latest edit, but belongs to an older revision
        if call == 2 {
            self.first_update_started.notify_one();
            self.release_first_update.notified().await;
        }

        // give each request a different vector so the test can tell which response was stored
        Ok(embedding_for_call(call))
    }
}

// fake provider that succeeds for the original transaction and search query, but fails for the edited text
struct FailingUpdatedEmbeddingProvider {
    call_count: AtomicUsize,
}

impl FailingUpdatedEmbeddingProvider {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EmbeddingProvider for FailingUpdatedEmbeddingProvider {
    async fn generate_embedding(
        &self,
        _http_client: &reqwest::Client,
        _openai_api_key: &str,
        embedding_text: &str,
    ) -> Result<Vec<f32>, (axum::http::StatusCode, String)> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        // fail when search tries to replace the original embedding with one for the edited transaction
        if embedding_text.contains("kind: Expense")
            && embedding_text.contains("Updated train commute")
        {
            return Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "simulated updated-transaction embedding failure".to_string(),
            ));
        }

        Ok(embedding_for_text(embedding_text))
    }
}

// make predictable test vectors so different descriptions can be compared without a real embedding model
fn embedding_for_text(embedding_text: &str) -> Vec<f32> {
    let embedding_text = embedding_text.to_ascii_lowercase();
    // use a different vector position for each description used by the edit tests
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

// make a different vector for each provider request, even when two requests have the same text
fn embedding_for_call(call: usize) -> Vec<f32> {
    let mut embedding = vec![0.0; 1536];
    embedding[call] = 1.0;
    embedding
}

// build the expense JSON used when creating or editing a transaction
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

// build an authenticated update request for the transaction being tested
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

// use the chosen fake provider, build the router, and register + log in a test user
async fn setup_user(
    embedding_provider: Arc<dyn EmbeddingProvider>,
) -> (AppState, axum::Router, uuid::Uuid, String) {
    let state = common::setup_app_state_with_embedding_provider(embedding_provider).await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

    (state, app, user_id, access_token)
}

// create a transaction through the API and return its ID for later edits and database checks
async fn add_transaction_and_get_id(
    app: &axum::Router,
    access_token: &str,
    description: &str,
) -> uuid::Uuid {
    // create a signed expense using the description that identifies this transaction in the test
    common::add_transaction(
        app,
        access_token,
        transaction_body(-50.00, "2026-03-01", "Transportation", description),
    )
    .await;

    // get this user's transactions so we can find the ID assigned by the database
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/api/transactions")
        .header("Authorization", format!("Bearer {}", access_token))
        .body(axum::body::Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    // read the response as transaction records
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let transactions: Vec<TransactionIdentity> = serde_json::from_slice(&body).unwrap();

    // find the transaction we just created by its description and return its ID
    transactions
        .into_iter()
        .find(|transaction| transaction.description.as_deref() == Some(description))
        .expect("created transaction was not returned by the API")
        .id
}

#[derive(Debug, PartialEq)]
// the stored text, vector, revision, and model that the tests compare with their expected values
struct StoredEmbedding {
    embedding_text: String,
    embedding: Vec<f32>,
    search_revision: i32,
    embedding_model: String,
}

// read the embedding directly from the database; return None when no embedding was stored
async fn stored_embedding(state: &AppState, transaction_id: uuid::Uuid) -> Option<StoredEmbedding> {
    let row = sqlx::query(
        "SELECT embedding_text, embedding, search_revision, embedding_model
         FROM transaction_embeddings
         WHERE transaction_id = $1",
    )
    .bind(transaction_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap();

    // convert the database row into the values used by the assertions
    row.map(|row| {
        let embedding_text: String = row.get("embedding_text");
        let embedding: Vector = row.get("embedding");
        let search_revision: i32 = row.get("search_revision");
        let embedding_model: String = row.get("embedding_model");
        StoredEmbedding {
            embedding_text,
            embedding: embedding.to_vec(),
            search_revision,
            embedding_model,
        }
    })
}

// read the transaction's current search revision so we can compare it with the stored embedding
async fn stored_transaction_revision(state: &AppState, transaction_id: uuid::Uuid) -> i32 {
    sqlx::query_scalar("SELECT search_revision FROM transactions WHERE id = $1")
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap()
}

// see if the embedding writer accepts matching transaction text and rejects text that no longer matches
#[tokio::test]
async fn test_guarded_embedding_store_rejects_stale_transaction_fields() {
    // set up a fake provider and a user, then create a transaction with its original embedding
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, user_id, access_token) = setup_user(provider).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    // generate an embedding with the same category, description, and revision as the stored transaction
    let current_embedding_text =
        "kind: Expense\n category: Transportation\n description: Original commute";
    let current_embedding = embedding_for_text(current_embedding_text);
    let current_transaction_embedding = TransactionEmbedding::generate(
        &state,
        "expense",
        Some("Transportation"),
        Some("Original commute"),
        1,
    )
    .await
    .unwrap();
    // try storing this matching embedding through the writer that checks the current transaction
    let stored = store_transaction_embedding_if_current(
        &state,
        transaction_id,
        user_id,
        current_transaction_embedding,
    )
    .await
    .unwrap();

    // the matching write should succeed and store the expected text, vector, revision, and model
    assert!(stored);
    assert_eq!(
        stored_embedding(&state, transaction_id).await.unwrap(),
        StoredEmbedding {
            embedding_text: current_embedding_text.to_string(),
            embedding: current_embedding.clone(),
            search_revision: 1,
            embedding_model: EXPECTED_EMBEDDING_MODEL.to_string(),
        }
    );

    // generate a response with the wrong category and description while keeping the same revision
    // this checks that the writer compares the text fields as well as the revision
    let stale_transaction_embedding = TransactionEmbedding::generate(
        &state,
        "expense",
        Some("Travel"),
        Some("Stale provider response"),
        1,
    )
    .await
    .unwrap();
    // try storing that response for the same transaction
    let stale_store = store_transaction_embedding_if_current(
        &state,
        transaction_id,
        user_id,
        stale_transaction_embedding,
    )
    .await
    .unwrap();

    // the write should be rejected and the previously stored embedding should stay unchanged
    assert!(!stale_store);
    assert_eq!(
        stored_embedding(&state, transaction_id).await.unwrap(),
        StoredEmbedding {
            embedding_text: current_embedding_text.to_string(),
            embedding: current_embedding,
            search_revision: 1,
            embedding_model: EXPECTED_EMBEDDING_MODEL.to_string(),
        }
    );
}

// see if editing only the amount and date preserves the existing search embedding
#[tokio::test]
async fn test_update_amount_and_date_does_not_regenerate_embedding() {
    // set up the user and create a transaction with a successful fake embedding
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;
    // save the original embedding so we can compare it after the edit
    let original_embedding = stored_embedding(&state, transaction_id).await.unwrap();
    assert_eq!(original_embedding.search_revision, 1);
    assert_eq!(original_embedding.embedding_model, EXPECTED_EMBEDDING_MODEL);

    // change the amount and date while keeping the category and description the same
    let request = update_request(
        &access_token,
        transaction_id,
        transaction_body(-75.00, "2026-03-02", "Transportation", "Original commute"),
    );
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);

    // read the transaction from the database to check that the financial edit was saved
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

    // check the updated amount and date, and confirm the description is unchanged
    assert_eq!(amount, rust_decimal::Decimal::new(-7500, 2));
    assert_eq!(date, chrono::NaiveDate::from_ymd_opt(2026, 3, 2).unwrap());
    assert_eq!(description.as_deref(), Some("Original commute"));
    // amount and date are not part of the embedding text, so the revision and embedding should stay the same
    // the provider should only have been called once when the transaction was created
    assert_eq!(stored_transaction_revision(&state, transaction_id).await, 1);
    assert_eq!(stored_embedding(&state, transaction_id).await.unwrap(), original_embedding);
    assert_eq!(provider.call_count(), 1);
}

// see if editing the category and description generates and stores a replacement embedding
#[tokio::test]
async fn test_update_semantic_fields_replaces_embedding() {
    // create the original transaction using a fake provider that succeeds
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    // edit the category and description, which are both included in the search embedding text
    let request = update_request(
        &access_token,
        transaction_id,
        transaction_body(-50.00, "2026-03-01", "Travel", "Updated train commute"),
    );
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);

    // read the replacement embedding from the database
    let embedding = stored_embedding(&state, transaction_id).await.unwrap();
    // check that the stored text and vector describe the edited transaction
    assert_eq!(
        embedding.embedding_text,
        "kind: Expense\n category: Travel\n description: Updated train commute"
    );
    assert_eq!(embedding.embedding, embedding_for_text("Updated train commute"));
    // both records should now be on revision 2, with one provider call for creation and one for the edit
    assert_eq!(embedding.search_revision, 2);
    assert_eq!(embedding.embedding_model, EXPECTED_EMBEDDING_MODEL);
    assert_eq!(stored_transaction_revision(&state, transaction_id).await, 2);
    assert_eq!(provider.call_count(), 2);
}

// see if a failed embedding request still saves the transaction edit and removes the old embedding
#[tokio::test]
async fn test_update_provider_failure_preserves_transaction_and_removes_stale_embedding() {
    // let creation succeed on the first provider call and make the edit fail on the second
    let provider = Arc::new(RecordingEmbeddingProvider::fails_on_call(2));
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    // change the category and description so the edit needs a replacement embedding
    let request = update_request(
        &access_token,
        transaction_id,
        transaction_body(-50.00, "2026-03-01", "Travel", "Updated train commute"),
    );
    let response = app.oneshot(request).await.unwrap();

    // the API should still return success because saving the transaction does not depend on the provider
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    // read the saved transaction to check what happened after the provider failure
    let row = sqlx::query(
        "SELECT category, description, search_revision FROM transactions WHERE id = $1",
    )
    .bind(transaction_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let category: Option<String> = row.get("category");
    let description: Option<String> = row.get("description");
    let search_revision: i32 = row.get("search_revision");

    // the category, description, and revision should reflect the edit
    assert_eq!(category.as_deref(), Some("Travel"));
    assert_eq!(description.as_deref(), Some("Updated train commute"));
    assert_eq!(search_revision, 2);
    // the old embedding describes the transaction before the edit, so it should no longer be stored
    assert!(stored_embedding(&state, transaction_id).await.is_none());
    assert_eq!(provider.call_count(), 2);
}

// see if a later search generates the missing embedding after an edit's provider request failed
#[tokio::test]
async fn test_search_backfills_embedding_after_update_provider_failure() {
    // make the second provider call fail so creation succeeds but the edit leaves no embedding
    let provider = Arc::new(RecordingEmbeddingProvider::fails_on_call(2));
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    // edit the category and description, then confirm the transaction saved without a replacement embedding
    let update = update_request(
        &access_token,
        transaction_id,
        transaction_body(-50.00, "2026-03-01", "Travel", "Updated train commute"),
    );
    let update_response = app.clone().oneshot(update).await.unwrap();
    assert_eq!(update_response.status(), axum::http::StatusCode::OK);
    assert!(stored_embedding(&state, transaction_id).await.is_none());

    // search for the edited description so the handler has to generate the missing embedding
    let search_body = serde_json::json!({
        "query": "updated commute",
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
    // read the search results and look for this transaction with its updated description
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

    // the edited transaction should appear in the results
    assert!(returned_updated_transaction);
    // check that search stored a replacement using the edited text and current revision
    let embedding = stored_embedding(&state, transaction_id).await.unwrap();
    assert_eq!(
        embedding.embedding_text,
        "kind: Expense\n category: Travel\n description: Updated train commute"
    );
    assert_eq!(embedding.embedding, embedding_for_text("Updated train commute"));
    assert_eq!(embedding.search_revision, 2);
    assert_eq!(embedding.embedding_model, EXPECTED_EMBEDDING_MODEL);
    assert_eq!(stored_transaction_revision(&state, transaction_id).await, 2);
    // expect four calls: creation, the failed edit, the replacement embedding, and the search query
    assert_eq!(provider.call_count(), 4);
}

// see if search excludes an old embedding when generating its replacement fails
#[tokio::test]
async fn test_search_excludes_stale_revision_when_backfill_fails() {
    // create a transaction with a provider that will later fail when asked to embed the edited text
    let provider = Arc::new(FailingUpdatedEmbeddingProvider::new());
    let (state, app, user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    // change the transaction directly to revision 2 while leaving the original embedding at revision 1
    sqlx::query(
        "UPDATE transactions
         SET category = $1, description = $2, search_revision = $3
         WHERE id = $4 AND user_id = $5",
    )
    .bind("Travel")
    .bind("Updated train commute")
    .bind(2_i32)
    .bind(transaction_id)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .unwrap();

    // store the original text and vector explicitly so the test starts with a known outdated embedding
    let stale_embedding_text =
        "kind: Expense\n category: Transportation\n description: Original commute";
    sqlx::query(
        "INSERT INTO transaction_embeddings (
            transaction_id, user_id, embedding_text, embedding, search_revision, embedding_model
         )
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (transaction_id) DO UPDATE SET
            user_id = EXCLUDED.user_id,
            embedding_text = EXCLUDED.embedding_text,
            embedding = EXCLUDED.embedding,
            search_revision = EXCLUDED.search_revision,
            embedding_model = EXCLUDED.embedding_model",
    )
    .bind(transaction_id)
    .bind(user_id)
    .bind(stale_embedding_text)
    .bind(Vector::from(embedding_for_text(stale_embedding_text)))
    .bind(1_i32)
    .bind(EXPECTED_EMBEDDING_MODEL)
    .execute(&state.pool)
    .await
    .unwrap();

    // search for the original text; the old vector would match if search ignored the revision mismatch
    let search_body = serde_json::json!({
        "query": "original commute",
        "limit": 5
    });
    // send the search request as the transaction owner
    let search_request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/transactions/search/semantic")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(search_body.to_string()))
        .unwrap();
    let search_response = app.oneshot(search_request).await.unwrap();

    assert_eq!(search_response.status(), axum::http::StatusCode::OK);
    // read the results and check whether the transaction was returned using its old embedding
    let body = search_response.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let transaction_id_string = transaction_id.to_string();
    let returned_stale_transaction = result["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| {
            result["transaction"]["id"].as_str() == Some(transaction_id_string.as_str())
        });

    // search should exclude this transaction because its replacement failed and the old embedding is outdated
    assert!(!returned_stale_transaction);
    // the database may still contain revision 1, but search must compare it with the transaction's revision 2
    let embedding = stored_embedding(&state, transaction_id).await.unwrap();
    assert_eq!(embedding.search_revision, 1);
    assert_eq!(embedding.embedding_model, EXPECTED_EMBEDDING_MODEL);
    assert_eq!(stored_transaction_revision(&state, transaction_id).await, 2);
    // expect creation, the failed replacement attempt, and the successful search-query embedding
    assert_eq!(provider.call_count(), 3);
}

// see if search checks the actual transaction owner when embedding rows have the wrong user IDs
#[tokio::test]
async fn test_semantic_search_rejects_poisoned_embedding_ownership() {
    // set up a successful fake provider and create the first user's transaction and embedding
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, first_user_id, first_access_token) = setup_user(provider).await;
    let first_transaction_id = add_transaction_and_get_id(
        &app,
        &first_access_token,
        "First user commute",
    )
    .await;

    // register + log in another user and create a separate transaction belonging to them
    let (second_username, second_password) = common::create_and_register_test_user(&app).await;
    let (second_user_id, second_access_token) =
        common::login_test_user(&app, &second_username, &second_password).await;
    let second_transaction_id = add_transaction_and_get_id(
        &app,
        &second_access_token,
        "Second user commute",
    )
    .await;

    // give the first user's embedding the second user's ID to simulate an incorrect stored owner
    // search should treat this embedding as unusable and regenerate it for the actual transaction owner
    sqlx::query(
        "UPDATE transaction_embeddings
         SET user_id = $1
         WHERE transaction_id = $2",
    )
    .bind(second_user_id)
    .bind(first_transaction_id)
    .execute(&state.pool)
    .await
    .unwrap();
    // give the second user's embedding the first user's ID to test the opposite case
    // search must not return this transaction just because its embedding claims to belong to the first user
    sqlx::query(
        "UPDATE transaction_embeddings
         SET user_id = $1
         WHERE transaction_id = $2",
    )
    .bind(first_user_id)
    .bind(second_transaction_id)
    .execute(&state.pool)
    .await
    .unwrap();

    // search as the first user; both descriptions match, so ownership must decide which transaction is returned
    let search_body = serde_json::json!({
        "query": "commute",
        "limit": 5
    });
    // include the first user's token and send the search request through the router
    let search_request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/transactions/search/semantic")
        .header("Authorization", format!("Bearer {}", first_access_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(search_body.to_string()))
        .unwrap();
    let search_response = app.oneshot(search_request).await.unwrap();

    assert_eq!(search_response.status(), axum::http::StatusCode::OK);
    // read the response and collect the returned transaction IDs for the ownership checks
    let body = search_response.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let returned_transaction_ids: Vec<&str> = result["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["transaction"]["id"].as_str())
        .collect();
    let first_transaction_id_string = first_transaction_id.to_string();
    let second_transaction_id_string = second_transaction_id.to_string();

    // the first user should see their own transaction, but must not see the second user's transaction
    assert!(returned_transaction_ids.contains(&first_transaction_id_string.as_str()));
    assert!(!returned_transaction_ids.contains(&second_transaction_id_string.as_str()));

    // read the first transaction's embedding owner to check that search repaired the stored user ID
    let repaired_embedding_user_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT user_id
         FROM transaction_embeddings
         WHERE transaction_id = $1",
    )
    .bind(first_transaction_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();

    // the repaired embedding should belong to the same user as the transaction
    assert_eq!(repaired_embedding_user_id, first_user_id);
}

// see if one user is prevented from editing a transaction belonging to another user
#[tokio::test]
async fn test_user_cannot_update_another_users_transaction() {
    // create the first user's transaction and its original embedding
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, _first_user_id, first_access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &first_access_token,
        "Original commute",
    )
    .await;

    // register + log in another user so we can try the edit using their token
    let (second_username, second_password) = common::create_and_register_test_user(&app).await;
    let (_second_user_id, second_access_token) =
        common::login_test_user(&app, &second_username, &second_password).await;
    // request an edit to the first user's transaction while authenticated as the second user
    let request = update_request(
        &second_access_token,
        transaction_id,
        transaction_body(-50.00, "2026-03-01", "Travel", "Unauthorized update"),
    );
    let response = app.oneshot(request).await.unwrap();

    // the transaction should be treated as not found for this user
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);

    // read the original transaction to confirm the rejected request did not change it
    let row = sqlx::query("SELECT description FROM transactions WHERE id = $1")
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let description: Option<String> = row.get("description");

    // the description and original embedding should remain, with no extra provider request
    assert_eq!(description.as_deref(), Some("Original commute"));
    assert!(stored_embedding(&state, transaction_id).await.is_some());
    assert_eq!(provider.call_count(), 1);
}

// see if an older edit's delayed embedding response is prevented from replacing a newer edit's result
#[tokio::test]
async fn test_delayed_update_cannot_overwrite_newer_embedding() {
    // create the original transaction with a fake provider that will pause the first edit
    let provider = Arc::new(BlockingEmbeddingProvider::new());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    // start the first edit in a separate task so we can send another edit while its provider request waits
    let first_request = update_request(
        &access_token,
        transaction_id,
        transaction_body(-50.00, "2026-03-01", "Travel", "First concurrent update"),
    );
    let first_app = app.clone();
    let first_update = tokio::spawn(async move { first_app.oneshot(first_request).await.unwrap() });

    // wait until the first edit reaches the provider, which confirms its financial write has already finished
    tokio::time::timeout(Duration::from_secs(2), provider.first_update_started.notified())
        .await
        .expect("first update did not reach embedding generation");

    // send a newer description and let that edit finish while the first provider request is still paused
    let second_request = update_request(
        &access_token,
        transaction_id,
        transaction_body(-50.00, "2026-03-01", "Travel", "Second concurrent update"),
    );
    // the timeout makes the test fail if the first edit is holding a database lock that blocks the second
    let second_response = tokio::time::timeout(
        Duration::from_secs(5),
        app.oneshot(second_request),
    )
    .await
    .expect("second update was blocked by the first provider call")
    .unwrap();
    assert_eq!(second_response.status(), axum::http::StatusCode::OK);

    // release the old provider response only after the newer edit has completed
    provider.release_first_update.notify_one();
    let first_response = tokio::time::timeout(Duration::from_secs(5), first_update)
        .await
        .expect("first update did not finish after provider release")
        .unwrap();
    assert_eq!(first_response.status(), axum::http::StatusCode::OK);

    // read the final transaction and embedding after both requests have finished
    let row = sqlx::query("SELECT description FROM transactions WHERE id = $1")
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let description: Option<String> = row.get("description");
    let embedding = stored_embedding(&state, transaction_id).await.unwrap();

    // the second edit should still be stored; the delayed first response must not replace it
    assert_eq!(description.as_deref(), Some("Second concurrent update"));
    assert_eq!(
        embedding.embedding_text,
        "kind: Expense\n category: Travel\n description: Second concurrent update"
    );
    assert_eq!(embedding.embedding, embedding_for_text("Second concurrent update"));
    // creation and the two edits should reach revision 3 and make three provider calls
    assert_eq!(embedding.search_revision, 3);
    assert_eq!(embedding.embedding_model, EXPECTED_EMBEDDING_MODEL);
    assert_eq!(stored_transaction_revision(&state, transaction_id).await, 3);
    assert_eq!(provider.call_count(), 3);
}

// see if an old embedding is rejected when later edits change the description and then change it back
#[tokio::test]
async fn test_delayed_update_cannot_overwrite_newer_same_fields_embedding() {
    // create the transaction with a fake provider that gives every request a different vector
    let provider = Arc::new(AbaBlockingEmbeddingProvider::new());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;

    // start the first edit with the description that we will also use in the final edit
    let first_request = update_request(
        &access_token,
        transaction_id,
        transaction_body(-50.00, "2026-03-01", "Travel", "Repeated concurrent update"),
    );
    let first_app = app.clone();
    let first_update = tokio::spawn(async move { first_app.oneshot(first_request).await.unwrap() });

    // wait for its provider request to pause so we know the next edits will finish before its response returns
    tokio::time::timeout(Duration::from_secs(2), provider.first_update_started.notified())
        .await
        .expect("first update did not reach embedding generation");

    // change the description to a different value while the first response is still waiting
    let intermediate_request = update_request(
        &access_token,
        transaction_id,
        transaction_body(
            -50.00,
            "2026-03-01",
            "Travel",
            "Intermediate concurrent update",
        ),
    );
    // this edit must finish without waiting for the paused provider request
    let intermediate_response = tokio::time::timeout(
        Duration::from_secs(5),
        app.clone().oneshot(intermediate_request),
    )
    .await
    .expect("intermediate update was blocked by the first provider call")
    .unwrap();
    assert_eq!(intermediate_response.status(), axum::http::StatusCode::OK);

    // change the description back to the first edit's value, creating a newer revision with the same text
    let final_request = update_request(
        &access_token,
        transaction_id,
        transaction_body(-50.00, "2026-03-01", "Travel", "Repeated concurrent update"),
    );
    let final_response = tokio::time::timeout(Duration::from_secs(5), app.oneshot(final_request))
        .await
        .expect("final update was blocked by the first provider call")
        .unwrap();
    assert_eq!(final_response.status(), axum::http::StatusCode::OK);

    // release the first response after the final edit; comparing text alone would now accept this older result
    provider.release_first_update.notify_one();
    let first_response = tokio::time::timeout(Duration::from_secs(5), first_update)
        .await
        .expect("first update did not finish after provider release")
        .unwrap();
    assert_eq!(first_response.status(), axum::http::StatusCode::OK);

    // read the final transaction revision and stored vector after all three edits complete
    let row = sqlx::query(
        "SELECT description, search_revision FROM transactions WHERE id = $1",
    )
    .bind(transaction_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let description: Option<String> = row.get("description");
    let search_revision: i32 = row.get("search_revision");
    let embedding = stored_embedding(&state, transaction_id).await.unwrap();

    // the final edit's revision and vector should remain, even though its description matches the delayed edit
    assert_eq!(description.as_deref(), Some("Repeated concurrent update"));
    assert_eq!(search_revision, 4);
    assert_eq!(embedding.embedding, embedding_for_call(4));
    assert_eq!(embedding.search_revision, 4);
    assert_eq!(embedding.embedding_model, EXPECTED_EMBEDDING_MODEL);
    // expect one provider request for creation and one for each of the three edits
    assert_eq!(provider.call_count(), 4);
}

// see if creating a transaction with a zero amount is rejected before any write or embedding request
#[tokio::test]
async fn test_create_rejects_zero_amount() {
    // set up a user with a fake provider that counts any embedding requests
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, user_id, access_token) = setup_user(provider.clone()).await;
    // send an authenticated create request with zero as the transaction amount
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

    // the API should reject the invalid amount with a bad-request response
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);

    // count this user's transactions to check that validation happened before writing to the database
    let transaction_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transactions WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();

    // no transaction should have been saved and the provider should not have been called
    assert_eq!(transaction_count, 0);
    assert_eq!(provider.call_count(), 0);
}

// see if an edit with a zero amount is rejected without changing the original transaction or embedding
#[tokio::test]
async fn test_update_rejects_zero_amount_and_preserves_existing_transaction() {
    // create a valid transaction first so we can check its values after the invalid edit
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;
    // try changing the amount to zero together with a new date, category, and description
    let request = update_request(
        &access_token,
        transaction_id,
        transaction_body(0.00, "2026-03-02", "Travel", "Invalid update"),
    );

    let response = app.oneshot(request).await.unwrap();

    // the API should reject the entire edit because its amount is invalid
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);

    // read the saved values directly from the database after the rejected request
    let row = sqlx::query("SELECT amount, date, category, description FROM transactions WHERE id = $1")
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let amount: rust_decimal::Decimal = row.get("amount");
    let date: chrono::NaiveDate = row.get("date");
    let category: Option<String> = row.get("category");
    let description: Option<String> = row.get("description");

    // all original values should remain, and the provider should only have been called during creation
    assert_eq!(amount, rust_decimal::Decimal::new(-5000, 2));
    assert_eq!(date, chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
    assert_eq!(category.as_deref(), Some("Transportation"));
    assert_eq!(description.as_deref(), Some("Original commute"));
    assert_eq!(provider.call_count(), 1);
}

// see if a negative expense amount is saved with its sign and does not regenerate an unchanged embedding
#[tokio::test]
async fn test_update_accepts_negative_amount() {
    // create the original expense and its embedding using the fake provider
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, app, _user_id, access_token) = setup_user(provider.clone()).await;
    let transaction_id = add_transaction_and_get_id(
        &app,
        &access_token,
        "Original commute",
    )
    .await;
    // edit the amount and date while keeping the category and description unchanged
    let request = update_request(
        &access_token,
        transaction_id,
        transaction_body(-10.00, "2026-03-02", "Transportation", "Original commute"),
    );

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);

    // read the saved amount, date, category, and description to check the accepted edit
    let row = sqlx::query("SELECT amount, date, category, description FROM transactions WHERE id = $1")
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let amount: rust_decimal::Decimal = row.get("amount");
    let date: chrono::NaiveDate = row.get("date");
    let category: Option<String> = row.get("category");
    let description: Option<String> = row.get("description");

    // the negative sign and new date should be saved without changing the descriptive fields
    assert_eq!(amount, rust_decimal::Decimal::new(-1000, 2));
    assert_eq!(date, chrono::NaiveDate::from_ymd_opt(2026, 3, 2).unwrap());
    assert_eq!(category.as_deref(), Some("Transportation"));
    assert_eq!(description.as_deref(), Some("Original commute"));
    // only creation should call the provider because amount and date are not part of the embedding text
    assert_eq!(provider.call_count(), 1);
}

// see if the database rejects a zero amount even when a write bypasses API validation
#[tokio::test]
async fn test_database_rejects_zero_transaction_amount() {
    // set up a user to own the transaction we will try inserting directly
    let provider = Arc::new(RecordingEmbeddingProvider::succeeds());
    let (state, _app, user_id, _access_token) = setup_user(provider).await;

    // insert a zero amount directly so this checks the database constraint rather than the HTTP handler
    let result = sqlx::query(
        "INSERT INTO transactions (user_id, amount, kind, category, date, description)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(user_id)
    .bind(rust_decimal::Decimal::ZERO)
    .bind("expense")
    .bind(Some("Transportation"))
    .bind(chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap())
    .bind(Some("Invalid direct insert"))
    .execute(&state.pool)
    .await;

    // the database should reject the insert because transaction amounts must be nonzero
    assert!(result.is_err());
}
