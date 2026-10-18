mod common;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use financetracker::bank_credentials::{BankCredentialCipher, CredentialContext};
use financetracker::embedding_worker::process_embedding_batch;
use financetracker::embeddings::EmbeddingProvider;
use financetracker::plaid::{
    ExchangedConnection, LinkToken, PlaidError, PlaidProvider, TransactionPage,
};
use financetracker::{AppState, build_app};
use http_body_util::BodyExt;
use mockall::{mock, predicate::eq};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::util::ServiceExt;
use uuid::Uuid;

const TEST_KEY: [u8; 32] = [61; 32];
const ACCESS_TOKEN: &str = "row-version-fake-access-token";
const SOURCE_ID: &str = "row-version-source";
const INITIAL_CURSOR: &str = "row-version-initial";
const INITIAL_ROW_VERSION: i64 = 1;

mock! {
    BankProvider {}

    #[async_trait]
    impl PlaidProvider for BankProvider {
        async fn create_link_token(&self, user_id: Uuid) -> Result<LinkToken, PlaidError>;
        async fn exchange_public_token(&self, public_token: &str) -> Result<ExchangedConnection, PlaidError>;
        async fn fetch_transactions(&self, access_token: &str, cursor: Option<String>) -> Result<TransactionPage, PlaidError>;
    }
}

// Count optional indexing work so a rejected edit cannot silently delete or replace an embedding.
struct RecordingEmbeddingProvider {
    calls: AtomicUsize,
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
        let mut embedding = vec![0.0; 1536];
        embedding[0] = 1.0;
        Ok(embedding)
    }
}

fn provider_transaction(amount: f64, date: &str) -> Value {
    json!({
        "transaction_id": SOURCE_ID,
        "account_id": "row-version-account",
        "amount": amount,
        "date": date,
        "name": "Imported coffee",
        "pending": false,
        "pending_transaction_id": null,
        "iso_currency_code": "CAD",
        "unofficial_currency_code": null,
        "personal_finance_category": {
            "primary": "FOOD_AND_DRINK",
            "detailed": "FOOD_AND_DRINK_COFFEE"
        }
    })
}

fn bank_provider(cursor: Option<String>, page: TransactionPage) -> Arc<dyn PlaidProvider> {
    let mut provider = MockBankProvider::new();
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(cursor))
        .times(1)
        .return_once(move |_, _| Ok(page));
    Arc::new(provider)
}

async fn request(
    state: &AppState,
    jwt: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    let body = body
        .map(|value| Body::from(value.to_string()))
        .unwrap_or_else(Body::empty);
    build_app(state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("Authorization", format!("Bearer {jwt}"))
                .header("Content-Type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

struct Fixture {
    state: AppState,
    jwt: String,
    connection_id: Uuid,
    transaction_id: Uuid,
    original: Value,
    embeddings: Arc<RecordingEmbeddingProvider>,
}

impl Fixture {
    async fn new(pool: PgPool) -> Self {
        let embeddings = Arc::new(RecordingEmbeddingProvider {
            calls: AtomicUsize::new(0),
        });
        let mut state = common::setup_app_state_with_embedding_provider(embeddings.clone()).await;
        state.pool = pool;
        state.bank_credential_cipher = Some(Arc::new(BankCredentialCipher::new(&TEST_KEY)));
        let app = build_app(state.clone());
        let (username, password) = common::create_and_register_test_user(&app).await;
        let (user_id, jwt) = common::login_test_user(&app, &username, &password).await;

        let provider_id = Uuid::new_v4().to_string();
        let encrypted = BankCredentialCipher::new(&TEST_KEY)
            .encrypt(
                ACCESS_TOKEN,
                &CredentialContext {
                    user_id,
                    provider: "plaid",
                    provider_id: &provider_id,
                },
            )
            .unwrap();
        let connection_id = sqlx::query_scalar(
            "INSERT INTO bank_connections (user_id, provider, provider_id, encrypted_access_token)
             VALUES ($1, 'plaid', $2, $3) RETURNING connection_id",
        )
        .bind(user_id)
        .bind(provider_id)
        .bind(encrypted)
        .fetch_one(&state.pool)
        .await
        .unwrap();

        state.plaid_provider = Some(bank_provider(
            None,
            TransactionPage {
                added: vec![provider_transaction(10.0, "2026-10-01")],
                modified: vec![],
                removed: vec![],
                next_cursor: INITIAL_CURSOR.to_string(),
                has_more: false,
            },
        ));
        let imported = request(
            &state,
            &jwt,
            "POST",
            &format!("/api/bank-connections/{connection_id}/import"),
            None,
        )
        .await;
        assert_eq!(imported.status(), StatusCode::OK);
        assert_eq!(response_json(imported).await, json!({ "imported": 1 }));

        let indexed = process_embedding_batch(&state, 10).await.unwrap();
        assert_eq!(indexed.indexed, 1);
        let response = request(&state, &jwt, "GET", "/api/transactions", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let rows = response_json(response).await;
        assert_eq!(rows.as_array().unwrap().len(), 1);
        let original = rows[0].clone();
        let transaction_id = original["id"].as_str().unwrap().parse().unwrap();

        Self {
            state,
            jwt,
            connection_id,
            transaction_id,
            original,
            embeddings,
        }
    }

    // A separate connection pool models another backend instance sharing the same database.
    async fn second_backend(&self) -> AppState {
        let mut state = self.state.clone();
        state.pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(self.state.pool.connect_options().as_ref().clone())
            .await
            .unwrap();
        state
    }

    async fn import(&self, state: &AppState) {
        let response = request(
            state,
            &self.jwt,
            "POST",
            &format!("/api/bank-connections/{}/import", self.connection_id),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await, json!({ "imported": 0 }));
    }

    async fn edit(&self, state: &AppState, input: Value) -> axum::response::Response {
        request(
            state,
            &self.jwt,
            "PUT",
            &format!("/api/transactions/{}", self.transaction_id),
            Some(input),
        )
        .await
    }

    // Snapshot every affected row, including source, cursor, override flags, and saved search data.
    async fn saved_state(&self) -> Value {
        sqlx::query_scalar(
            "SELECT jsonb_build_object(
                'transaction', to_jsonb(t), 'source', to_jsonb(d), 'connection', to_jsonb(c),
                'embedding', (SELECT to_jsonb(e) FROM transaction_embeddings e WHERE e.transaction_id = t.id))
             FROM transactions t
             JOIN imported_transaction_details d ON d.transaction_id = t.id
             JOIN bank_connections c ON c.connection_id = d.connection_id
             WHERE t.id = $1",
        )
        .bind(self.transaction_id).fetch_one(&self.state.pool).await.unwrap()
    }
}

fn edit_input(transaction: &Value, description: &str, expected_version: i64) -> Value {
    json!({
        "amount": transaction["amount"],
        "kind": transaction["kind"],
        "category": transaction["category"],
        "date": transaction["date"],
        "description": description,
        "expected_version": expected_version
    })
}

// Keep each import and indexing scan isolated from other tests running in parallel.
#[sqlx::test(migrations = "backend/migrations")]
async fn stale_edit_after_money_only_import_is_rejected_across_backend_instances(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    let mut importer = fixture.second_backend().await;
    importer.plaid_provider = Some(bank_provider(
        Some(INITIAL_CURSOR.to_string()),
        TransactionPage {
            added: vec![],
            modified: vec![provider_transaction(12.0, "2026-10-02")],
            removed: vec![],
            next_cursor: "row-version-corrected".to_string(),
            has_more: false,
        },
    ));
    fixture.import(&importer).await;
    let before = fixture.saved_state().await;
    assert_eq!(before["transaction"]["amount"], json!(-12.0));
    assert_eq!(before["transaction"]["date"], "2026-10-02");
    assert_eq!(before["transaction"]["search_revision"], json!(1));
    let calls = fixture.embeddings.calls.load(Ordering::SeqCst);

    // The draft was opened at the initial version; its money/date still predate the other import.
    let response = fixture
        .edit(
            &fixture.state,
            edit_input(
                &fixture.original,
                "Description from the old draft",
                INITIAL_ROW_VERSION,
            ),
        )
        .await;
    let after = fixture.saved_state().await;
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "stale save changed amount/date to {}/{} and description to {}",
        after["transaction"]["amount"],
        after["transaction"]["date"],
        after["transaction"]["description"]
    );
    assert_eq!(
        after, before,
        "rejection must preserve financial, source, cursor, override, and embedding rows"
    );
    assert_eq!(fixture.embeddings.calls.load(Ordering::SeqCst), calls);

    // Explicitly rereading and reviewing the corrected values permits a fresh save.
    let response = request(
        &fixture.state,
        &fixture.jwt,
        "GET",
        "/api/transactions",
        None,
    )
    .await;
    let current = response_json(response).await[0].clone();
    let current_version = current["row_version"].as_i64().unwrap();
    assert!(current_version > INITIAL_ROW_VERSION);
    let accepted = fixture
        .edit(
            &fixture.state,
            edit_input(&current, "Reviewed description", current_version),
        )
        .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let saved = fixture.saved_state().await;
    assert!(saved["transaction"]["row_version"].as_i64().unwrap() > current_version);
    assert_eq!(saved["transaction"]["amount"], json!(-12.0));
    assert_eq!(saved["transaction"]["date"], "2026-10-02");
    assert_eq!(saved["transaction"]["description"], "Reviewed description");
}

#[sqlx::test(migrations = "backend/migrations")]
async fn competing_edits_with_the_same_version_have_exactly_one_winner(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    let second = fixture.second_backend().await;
    let calls = fixture.embeddings.calls.load(Ordering::SeqCst);
    let (first, second) = tokio::join!(
        fixture.edit(
            &fixture.state,
            edit_input(&fixture.original, "First tab", INITIAL_ROW_VERSION)
        ),
        fixture.edit(
            &second,
            edit_input(&fixture.original, "Second tab", INITIAL_ROW_VERSION)
        ),
    );
    let mut statuses = vec![first.status().as_u16(), second.status().as_u16()];
    statuses.sort_unstable();
    assert_eq!(
        statuses,
        vec![200, 409],
        "both backend instances must not accept the same draft version"
    );
    let winner = if first.status() == StatusCode::OK {
        "First tab"
    } else {
        "Second tab"
    };
    let saved = fixture.saved_state().await;
    assert_eq!(saved["transaction"]["description"], winner);
    assert_eq!(fixture.embeddings.calls.load(Ordering::SeqCst), calls + 1);
}

#[sqlx::test(migrations = "backend/migrations")]
async fn edit_without_expected_version_cannot_bypass_the_stale_save_check(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    let before = fixture.saved_state().await;
    let calls = fixture.embeddings.calls.load(Ordering::SeqCst);
    let mut input = edit_input(&fixture.original, "Unversioned edit", INITIAL_ROW_VERSION);
    input.as_object_mut().unwrap().remove("expected_version");
    let response = fixture.edit(&fixture.state, input).await;
    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "an edit request must supply the version on which its draft was based"
    );
    assert_eq!(fixture.saved_state().await, before);
    assert_eq!(fixture.embeddings.calls.load(Ordering::SeqCst), calls);
}

#[sqlx::test(migrations = "backend/migrations")]
async fn list_and_semantic_results_expose_the_initial_transaction_version(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    let response = request(
        &fixture.state,
        &fixture.jwt,
        "POST",
        "/api/transactions/search/semantic",
        Some(json!({ "query": "coffee", "summary": false })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let results = response_json(response).await;
    assert_eq!(results["transactions"].as_array().unwrap().len(), 1);
    assert_eq!(
        json!({
            "list": fixture.original["row_version"],
            "search": results["transactions"][0]["transaction"]["row_version"]
        }),
        json!({ "list": INITIAL_ROW_VERSION, "search": INITIAL_ROW_VERSION }),
        "both places that can open an editor must return its saved row version"
    );
}

#[sqlx::test(migrations = "backend/migrations")]
async fn import_removal_invalidates_an_already_open_edit(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    let mut importer = fixture.second_backend().await;
    importer.plaid_provider = Some(bank_provider(
        Some(INITIAL_CURSOR.to_string()),
        TransactionPage {
            added: vec![],
            modified: vec![],
            removed: vec![json!({ "transaction_id": SOURCE_ID })],
            next_cursor: "row-version-removed".to_string(),
            has_more: false,
        },
    ));
    fixture.import(&importer).await;
    let before = fixture.saved_state().await;
    assert_eq!(before["transaction"]["status"], "removed");
    let calls = fixture.embeddings.calls.load(Ordering::SeqCst);
    let response = fixture
        .edit(
            &fixture.state,
            edit_input(
                &fixture.original,
                "Stale edit of a removed transaction",
                INITIAL_ROW_VERSION,
            ),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(fixture.saved_state().await, before);
    assert_eq!(fixture.embeddings.calls.load(Ordering::SeqCst), calls);
}

#[sqlx::test(migrations = "backend/migrations")]
async fn revert_that_only_clears_override_flags_invalidates_an_older_edit(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    let first = fixture
        .edit(
            &fixture.state,
            edit_input(&fixture.original, "User note", INITIAL_ROW_VERSION),
        )
        .await;
    assert_eq!(first.status(), StatusCode::OK);
    let second = fixture
        .edit(
            &fixture.state,
            edit_input(&fixture.original, "Imported coffee", 2),
        )
        .await;
    assert_eq!(second.status(), StatusCode::OK);
    let before = fixture.saved_state().await;
    assert_eq!(before["transaction"]["description_overridden"], true);
    let calls = fixture.embeddings.calls.load(Ordering::SeqCst);

    // Revert still changes future import behavior when the visible description already matches its source.
    let reverted = request(
        &fixture.state,
        &fixture.jwt,
        "POST",
        &format!("/api/transactions/{}/revert", fixture.transaction_id),
        None,
    )
    .await;
    assert_eq!(reverted.status(), StatusCode::OK);
    let after_revert = fixture.saved_state().await;
    assert_eq!(after_revert["transaction"]["description_overridden"], false);
    assert_eq!(
        after_revert["transaction"]["search_revision"],
        before["transaction"]["search_revision"]
    );
    assert_eq!(fixture.embeddings.calls.load(Ordering::SeqCst), calls);

    let stale = fixture
        .edit(
            &fixture.state,
            edit_input(&fixture.original, "Draft from before Revert", 3),
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(fixture.saved_state().await, after_revert);
    assert_eq!(fixture.embeddings.calls.load(Ordering::SeqCst), calls);
}
