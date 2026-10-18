mod common;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use financetracker::bank_credentials::{BankCredentialCipher, CredentialContext};
use financetracker::plaid::{
    ExchangedConnection, LinkToken, PlaidError, PlaidProvider, TransactionPage,
};
use financetracker::{AppState, build_app};
use http_body_util::BodyExt;
use mockall::{mock, predicate::eq};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use std::time::Duration;
use tower::util::ServiceExt;
use uuid::Uuid;

const TEST_KEY: [u8; 32] = [73; 32];
const BANK_ACCESS_TOKEN: &str = "import-revert-test-token";

mock! {
    BankProvider {}

    #[async_trait]
    impl PlaidProvider for BankProvider {
        async fn create_link_token(&self, user_id: Uuid) -> Result<LinkToken, PlaidError>;
        async fn exchange_public_token(&self, public_token: &str) -> Result<ExchangedConnection, PlaidError>;
        async fn fetch_transactions(&self, access_token: &str, cursor: Option<String>) -> Result<TransactionPage, PlaidError>;
    }
}

// use a separate named connection for each request so the test can observe its database wait.
async fn request_state(state: &AppState, application_name: &str) -> AppState {
    let mut request_state = state.clone();
    let options = state.pool.connect_options().as_ref().clone()
        .application_name(application_name);
    request_state.pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    request_state
}

// bound both requests so a failed synchronization cannot leave a test task or database lock running.
async fn post_request(state: AppState, access_token: String, uri: String) -> axum::response::Response {
    let app = build_app(state);
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("Authorization", format!("Bearer {access_token}"))
        .body(Body::empty())
        .unwrap();

    let response = tokio::time::timeout(Duration::from_secs(30), app.oneshot(request))
        .await
        .expect("the import and Revert requests must finish after the table lock is released")
        .unwrap();
    response
}

// wait for a real request to reach the held provenance table, rather than guessing with a sleep.
async fn wait_for_provenance_gate(
    pool: &PgPool,
    application_name: &str,
    gate_pid: i32,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM pg_stat_activity
                WHERE datname = current_database() AND application_name = $1
                  AND $2 = ANY(pg_blocking_pids(pid)))",
        )
        .bind(application_name)
        .bind(gate_pid)
        .fetch_one(pool)
        .await?;
        if waiting {
            return Ok(());
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "{application_name} did not reach the provenance table lock"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// use the test module
#[cfg(test)]
mod import_revert_concurrency_tests {
    use super::*;

    // test that an import and revert can both finish while preserving the corrected money and original source
    #[sqlx::test(migrations = "backend/migrations")]
    async fn test_import_and_revert_do_not_deadlock(pool: PgPool) {
        // register a user in this test's database with a fake embedding provider.
        let mut state = common::setup_app_state().await;
        state.pool = pool;
        state.bank_credential_cipher = Some(Arc::new(BankCredentialCipher::new(&TEST_KEY)));
        let app = build_app(state.clone());
        let (username, password) = common::create_and_register_test_user(&app).await;
        let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // save a fake encrypted bank credential and an imported transaction with user overrides.
        let provider_id = Uuid::new_v4().to_string();
        let encrypted_access_token = BankCredentialCipher::new(&TEST_KEY)
            .encrypt(
                BANK_ACCESS_TOKEN,
                &CredentialContext {
                    user_id,
                    provider: "plaid",
                    provider_id: &provider_id,
                },
            )
            .unwrap();
        let connection_id: Uuid = sqlx::query_scalar(
            "INSERT INTO bank_connections
                (user_id, provider, provider_id, encrypted_access_token, progress_cursor)
             VALUES ($1, 'plaid', $2, $3, 'before') RETURNING connection_id",
        )
        .bind(user_id)
        .bind(provider_id)
        .bind(encrypted_access_token)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        let transaction_id: Uuid = sqlx::query_scalar(
            "INSERT INTO transactions
                (user_id, amount, kind, category, description, date, origin,
                 kind_overridden, category_overridden, description_overridden)
             VALUES ($1, -10.00, 'income', 'My category', 'My note', '2026-10-01', 'imported',
                     TRUE, TRUE, TRUE)
             RETURNING id",
        )
        .bind(user_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO imported_transaction_details
                (transaction_id, connection_id, provider, provider_transaction_id,
                 provider_account_id, source_status, original_payload,
                 inferred_kind, inferred_category, inferred_description)
             VALUES ($1, $2, 'plaid', 'coffee', 'account', 'posted', $3,
                     'expense', 'Food', 'Original coffee')",
        )
        .bind(transaction_id)
        .bind(connection_id)
        .bind(json!({ "transaction_id": "coffee", "amount": 10, "date": "2026-10-01" }))
        .execute(&state.pool)
        .await
        .unwrap();
        let source_before: Value = sqlx::query_scalar(
            "SELECT to_jsonb(d) FROM imported_transaction_details d WHERE transaction_id = $1",
        )
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();

        // the fake Plaid response corrects money and date while the saved overrides still apply.
        let mut bank_provider = MockBankProvider::new();
        bank_provider
            .expect_fetch_transactions()
            .with(eq(BANK_ACCESS_TOKEN), eq(Some("before".to_string())))
            .times(1)
            .return_once(|_, _| Ok(TransactionPage {
                added: vec![],
                modified: vec![json!({
                    "transaction_id": "coffee",
                    "account_id": "account",
                    "amount": 12,
                    "date": "2026-10-02",
                    "name": "Corrected coffee",
                    "pending": false,
                    "pending_transaction_id": null,
                    "iso_currency_code": "CAD",
                    "unofficial_currency_code": null,
                    "personal_finance_category": {
                        "primary": "FOOD_AND_DRINK",
                        "detailed": "COFFEE"
                    }
                })],
                removed: vec![],
                next_cursor: "after".to_string(),
                has_more: false,
            }));
        state.plaid_provider = Some(Arc::new(bank_provider));
        let import_state = request_state(&state, "import-revert-import").await;
        let revert_state = request_state(&state, "import-revert-revert").await;

        // pause provenance reads so import has its connection lock before revert reaches the same table.
        // the old revert path also holds the transaction here, creating the opposite row-lock order.
        let mut provenance_gate = state.pool.begin().await.unwrap();
        let gate_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *provenance_gate)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE imported_transaction_details IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *provenance_gate)
            .await
            .unwrap();
        let import_request = tokio::spawn(post_request(
            import_state,
            access_token.clone(),
            format!("/api/bank-connections/{connection_id}/import"),
        ));
        let import_wait = wait_for_provenance_gate(
            &state.pool,
            "import-revert-import",
            gate_pid,
        )
        .await;
        let revert_request = tokio::spawn(post_request(
            revert_state,
            access_token,
            format!("/api/transactions/{transaction_id}/revert"),
        ));
        let revert_wait = wait_for_provenance_gate(
            &state.pool,
            "import-revert-revert",
            gate_pid,
        )
        .await;

        // release the table lock and finish both bounded requests before asserting any observations.
        provenance_gate.rollback().await.unwrap();
        let (import_response, revert_response) = tokio::join!(import_request, revert_request);
        import_wait.expect("import must reach the controlled overlap");
        revert_wait.expect("Revert must reach the controlled overlap");
        let import_response = import_response.unwrap();
        let revert_response = revert_response.unwrap();
        let import_status = import_response.status();
        let revert_status = revert_response.status();
        let import_body = import_response.into_body().collect().await.unwrap();
        let import_body_bytes = import_body.to_bytes();
        let revert_body = revert_response.into_body().collect().await.unwrap();
        let revert_body_bytes = revert_body.to_bytes();
        println!("import status: {import_status}; Revert status: {revert_status}");
        assert_eq!(
            (import_status, revert_status),
            (StatusCode::OK, StatusCode::OK),
            "import: {}; Revert: {}",
            String::from_utf8_lossy(&import_body_bytes),
            String::from_utf8_lossy(&revert_body_bytes),
        );
        let import_result: Value = serde_json::from_slice(&import_body_bytes).unwrap();
        assert_eq!(import_result, json!({ "imported": 0 }));

        // both writes must survive: import saves the correction and cursor, then revert releases overrides.
        let saved: Value = sqlx::query_scalar(
            "SELECT jsonb_build_object(
                'transaction', to_jsonb(t), 'source', to_jsonb(d),
                'cursor', c.progress_cursor, 'last_synced_at', c.last_synced_at,
                'embedding_revision', e.search_revision, 'embedding_text', e.embedding_text)
             FROM transactions t
             JOIN imported_transaction_details d ON d.transaction_id = t.id
             JOIN bank_connections c ON c.connection_id = d.connection_id
             LEFT JOIN transaction_embeddings e ON e.transaction_id = t.id
             WHERE t.id = $1",
        )
        .bind(transaction_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(saved["transaction"]["amount"], json!(-12.0));
        assert_eq!(saved["transaction"]["date"], "2026-10-02");
        assert_eq!(saved["transaction"]["kind"], "expense");
        assert_eq!(saved["transaction"]["category"], "Food");
        assert_eq!(saved["transaction"]["description"], "Original coffee");
        assert_eq!(saved["transaction"]["kind_overridden"], false);
        assert_eq!(saved["transaction"]["category_overridden"], false);
        assert_eq!(saved["transaction"]["description_overridden"], false);
        assert_eq!(saved["transaction"]["row_version"], 3);
        assert_eq!(saved["transaction"]["search_revision"], 2);
        assert_eq!(saved["embedding_revision"], saved["transaction"]["search_revision"]);
        assert!(saved["embedding_text"].as_str().unwrap().contains("Original coffee"));
        assert_eq!(saved["source"], source_before);
        assert_eq!(saved["cursor"], "after");
        assert!(!saved["last_synced_at"].is_null());
    }
}
