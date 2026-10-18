mod common;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, NaiveDate, Utc};
use financetracker::bank_credentials::{BankCredentialCipher, CredentialContext};
use financetracker::plaid::{
    ExchangedConnection, LinkToken, PlaidClient, PlaidError, PlaidProvider, TransactionPage,
};
use financetracker::{AppState, build_app};
use http_body_util::BodyExt;
use mockall::{mock, predicate::eq};
use serde_json::{Value, json};
use sqlx::types::Decimal;
use std::sync::Arc;
use std::time::Duration;
use tower::util::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_KEY: [u8; 32] = [37; 32];
const ACCESS_TOKEN: &str = "access-sandbox-import-test-token";

// exercise the normal HTTP handler while keeping every Plaid response under test control
mock! {
    BankProvider {}

    #[async_trait]
    impl PlaidProvider for BankProvider {
        async fn create_link_token(&self, user_id: Uuid) -> Result<LinkToken, PlaidError>;
        async fn exchange_public_token(&self, public_token: &str) -> Result<ExchangedConnection, PlaidError>;
        async fn fetch_transactions(&self, access_token: &str, cursor: Option<String>) -> Result<TransactionPage, PlaidError>;
    }
}

// keep the raw provider fixture so the test can check both normalized money and untouched source data
fn provider_transaction(id: &str, amount: f64, date: &str, name: &str) -> Value {
    json!({
        "transaction_id": id,
        "account_id": "test-provider-account",
        "amount": amount,
        "date": date,
        "name": name,
        "pending": false,
        "pending_transaction_id": null,
        "iso_currency_code": "CAD",
        "unofficial_currency_code": null,
        "personal_finance_category": {
            "primary": "FOOD_AND_DRINK",
            "detailed": "FOOD_AND_DRINK_GROCERIES",
            "confidence_level": "VERY_HIGH"
        },
        "extra_provider_data": { "retain_this": true }
    })
}

#[derive(sqlx::FromRow)]
struct ImportedTransaction {
    user_id: Uuid,
    amount: Decimal,
    kind: String,
    category: Option<String>,
    date: NaiveDate,
    description: Option<String>,
    currency: Option<String>,
    origin: String,
    status: String,
    connection_id: Uuid,
    provider: String,
    provider_transaction_id: String,
    provider_account_id: String,
    source_status: String,
    original_payload: Value,
    inferred_kind: String,
    inferred_category: Option<String>,
    inferred_description: Option<String>,
}

// the frontend supplies its saved connection ID; the backend must fetch and save the returned transactions
#[tokio::test]
async fn test_import_calls_plaid_and_saves_two_transactions_for_connection_owner() {
    let mut state = common::setup_app_state().await;
    state.bank_credential_cipher = Some(Arc::new(BankCredentialCipher::new(&TEST_KEY)));
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, jwt) = common::login_test_user(&app, &username, &password).await;

    // seed a real encrypted credential; the provider must receive its decrypted value, not client input
    let provider_id = Uuid::new_v4().to_string();
    let context = CredentialContext {
        user_id,
        provider: "plaid",
        provider_id: &provider_id,
    };
    let encrypted = BankCredentialCipher::new(&TEST_KEY)
        .encrypt(ACCESS_TOKEN, &context)
        .unwrap();
    let connection_id: Uuid = sqlx::query_scalar(
        "INSERT INTO bank_connections (user_id, provider, provider_id, encrypted_access_token)
         VALUES ($1, 'plaid', $2, $3) RETURNING connection_id",
    )
    .bind(user_id)
    .bind(&provider_id)
    .bind(encrypted)
    .fetch_one(&state.pool)
    .await
    .unwrap();

    let expected = vec![
        provider_transaction(
            "test-source-one",
            12.50,
            "2026-10-01",
            "Test grocery purchase",
        ),
        provider_transaction(
            "test-source-two",
            21.75,
            "2026-10-02",
            "Another grocery purchase",
        ),
    ];
    let added = expected.clone();
    let mut provider = MockBankProvider::new();
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .returning(move |_, _| {
            Ok(TransactionPage {
                added: added.clone(),
                modified: vec![],
                removed: vec![],
                next_cursor: "saved-import-progress".to_string(),
                has_more: false,
            })
        });
    state.plaid_provider = Some(Arc::new(provider));

    // import is requested with the connection ID and existing login; no Plaid credential comes from the frontend
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/bank-connections/{connection_id}/import"))
        .header("Authorization", format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();
    let response = build_app(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"imported": 2})
    );

    // both normalized rows and their original provider data must be saved under the authenticated owner
    let imported = sqlx::query_as::<_, ImportedTransaction>(
        "SELECT t.user_id, t.amount, t.kind, t.category, t.date, t.description, t.currency, t.origin,
                t.status, d.connection_id, d.provider, d.provider_transaction_id, d.provider_account_id,
                d.source_status, d.original_payload, d.inferred_kind, d.inferred_category, d.inferred_description
         FROM transactions t JOIN imported_transaction_details d ON d.transaction_id = t.id
         WHERE d.connection_id = $1 ORDER BY d.provider_transaction_id",
    )
    .bind(connection_id).fetch_all(&state.pool).await.unwrap();
    assert_eq!(imported.len(), 2);
    for (index, row) in imported.iter().enumerate() {
        assert_eq!(row.user_id, user_id);
        assert_eq!(row.connection_id, connection_id);
        assert_eq!(row.provider, "plaid");
        assert_eq!(
            row.provider_transaction_id,
            expected[index]["transaction_id"].as_str().unwrap()
        );
        assert_eq!(row.provider_account_id, "test-provider-account");
        assert_eq!(row.origin, "imported");
        assert_eq!(row.status, "posted");
        assert_eq!(row.source_status, "posted");
        assert_eq!(row.currency.as_deref(), Some("CAD"));
        assert_eq!(
            row.date.to_string(),
            expected[index]["date"].as_str().unwrap()
        );
        assert_eq!(row.description.as_deref(), expected[index]["name"].as_str());
        assert_eq!(row.original_payload, expected[index]);
        assert_eq!(row.kind, "expense");
        assert_eq!(row.inferred_kind, row.kind);
        assert_eq!(row.inferred_category, row.category);
        assert_eq!(row.inferred_description, row.description);
    }

    // Plaid expenses use the opposite sign; FinanceTracker must retain exact decimal outflows
    assert_eq!(imported[0].amount, Decimal::new(-1250, 2));
    assert_eq!(imported[1].amount, Decimal::new(-2175, 2));
    let (cursor, last_synced_at): (Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT progress_cursor, last_synced_at FROM bank_connections WHERE connection_id = $1",
    )
    .bind(connection_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(cursor.as_deref(), Some("saved-import-progress"));
    assert!(last_synced_at.is_some());
}

// prove that the real HTTP client calls Plaid with the saved credential and resume position
#[tokio::test]
async fn test_plaid_client_fetches_transaction_page() {
    let server = MockServer::start().await;
    let added = vec![provider_transaction(
        "provider-http-transaction",
        12.50,
        "2026-10-01",
        "Test purchase",
    )];
    Mock::given(method("POST"))
        .and(path("/transactions/sync"))
        .and(header("PLAID-CLIENT-ID", "test-client-id"))
        .and(header("PLAID-SECRET", "test-sandbox-secret"))
        .and(body_json(
            json!({"access_token": ACCESS_TOKEN, "cursor": "previous-progress"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "added": added, "modified": [], "removed": [],
            "next_cursor": "next-progress", "has_more": false,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = PlaidClient::new(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap(),
        &server.uri(),
        "test-client-id".to_string(),
        "test-sandbox-secret".to_string(),
        vec!["CA".to_string()],
    );
    let page = client
        .fetch_transactions(ACCESS_TOKEN, Some("previous-progress".to_string()))
        .await
        .unwrap();
    assert_eq!(page.added, added);
    assert!(page.modified.is_empty());
    assert!(page.removed.is_empty());
    assert_eq!(page.next_cursor, "next-progress");
    assert!(!page.has_more);
}
struct ImportFixture {
    state: AppState,
    user_id: Uuid,
    jwt: String,
    connection_id: Uuid,
}

impl ImportFixture {
    async fn new() -> Self {
        let mut state = common::setup_app_state().await;
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
        Self {
            state,
            user_id,
            jwt,
            connection_id,
        }
    }

    async fn import(&self) -> (StatusCode, Value) {
        import_request(&self.state, self.connection_id, Some(&self.jwt)).await
    }

    async fn saved_connection(&self) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(c) FROM bank_connections c WHERE connection_id = $1")
            .bind(self.connection_id)
            .fetch_one(&self.state.pool)
            .await
            .unwrap()
    }

    async fn saved_transactions(&self) -> Vec<Value> {
        sqlx::query_scalar(
            "SELECT jsonb_build_object('transaction', to_jsonb(t), 'source', to_jsonb(d))
             FROM transactions t JOIN imported_transaction_details d ON d.transaction_id = t.id
             WHERE d.connection_id = $1 ORDER BY t.id",
        )
        .bind(self.connection_id)
        .fetch_all(&self.state.pool)
        .await
        .unwrap()
    }

    async fn saved_progress(&self) -> (Option<String>, Option<DateTime<Utc>>, i64) {
        sqlx::query_as(
            "SELECT progress_cursor, last_synced_at,
                    (SELECT COUNT(*) FROM imported_transaction_details WHERE connection_id = $1)
             FROM bank_connections WHERE connection_id = $1",
        )
        .bind(self.connection_id)
        .fetch_one(&self.state.pool)
        .await
        .unwrap()
    }
}

// share only request mechanics; each test still states its provider input and database expectations
async fn import_request(
    state: &AppState,
    connection_id: Uuid,
    jwt: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/bank-connections/{connection_id}/import"));
    if let Some(jwt) = jwt {
        request = request.header("Authorization", format!("Bearer {jwt}"));
    }
    let response = build_app(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

fn transaction_page(
    added: Vec<Value>,
    modified: Vec<Value>,
    removed: Vec<Value>,
    cursor: &str,
    has_more: bool,
) -> TransactionPage {
    TransactionPage {
        added,
        modified,
        removed,
        next_cursor: cursor.to_string(),
        has_more,
    }
}

fn purchase(id: &str) -> Value {
    provider_transaction(id, 12.50, "2026-10-01", "Original groceries")
}

#[tokio::test]
async fn test_import_requires_login_and_connection_ownership_before_calling_plaid() {
    let mut fixture = ImportFixture::new().await;
    fixture.state.plaid_provider = Some(Arc::new(MockBankProvider::new()));

    // no provider calls are configured: unauthorized or unknown connections must stop before Plaid
    assert_eq!(
        import_request(&fixture.state, fixture.connection_id, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        import_request(&fixture.state, fixture.connection_id, Some("invalid-jwt"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        import_request(&fixture.state, Uuid::new_v4(), Some(&fixture.jwt))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    let app = build_app(fixture.state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (_, other_jwt) = common::login_test_user(&app, &username, &password).await;
    assert_eq!(
        import_request(&fixture.state, fixture.connection_id, Some(&other_jwt))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(fixture.saved_progress().await, (None, None, 0));
}

#[tokio::test]
async fn test_import_checks_configuration_connection_status_and_encrypted_credential() {
    let mut fixture = ImportFixture::new().await;
    assert_eq!(fixture.import().await.0, StatusCode::SERVICE_UNAVAILABLE);
    fixture.state.plaid_provider = Some(Arc::new(MockBankProvider::new()));
    fixture.state.bank_credential_cipher = None;
    assert_eq!(fixture.import().await.0, StatusCode::SERVICE_UNAVAILABLE);
    fixture.state.bank_credential_cipher = Some(Arc::new(BankCredentialCipher::new(&TEST_KEY)));

    // inactive connections and unreadable credentials cannot be used for an external request
    for status in ["needs_reconnection", "disconnected"] {
        sqlx::query("UPDATE bank_connections SET status = $2 WHERE connection_id = $1")
            .bind(fixture.connection_id)
            .bind(status)
            .execute(&fixture.state.pool)
            .await
            .unwrap();
        assert_eq!(fixture.import().await.0, StatusCode::CONFLICT);
    }
    sqlx::query("UPDATE bank_connections SET status = 'active', encrypted_access_token = $2 WHERE connection_id = $1")
        .bind(fixture.connection_id).bind(vec![1u8, 2, 3]).execute(&fixture.state.pool).await.unwrap();
    assert_eq!(fixture.import().await.0, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(fixture.saved_progress().await, (None, None, 0));
}

#[tokio::test]
async fn test_import_fetches_all_pages_and_repeated_ids_do_not_create_duplicates() {
    let mut fixture = ImportFixture::new().await;
    let mut provider = MockBankProvider::new();
    let mut sequence = mockall::Sequence::new();
    let mut income = purchase("income");
    income["amount"] = json!(-120.25);
    income["personal_finance_category"] = json!({"primary":"INCOME", "detailed":"INCOME_WAGES"});
    let mut transfer = purchase("transfer");
    transfer["personal_finance_category"] =
        json!({"primary":"TRANSFER_OUT", "detailed":"TRANSFER_OUT_ACCOUNT_TRANSFER"});
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(move |_, _| {
            Ok(transaction_page(
                vec![income.clone()],
                vec![],
                vec![],
                "page-one",
                true,
            ))
        });
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some("page-one".to_string())))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(move |_, _| {
            Ok(transaction_page(
                vec![transfer.clone()],
                vec![],
                vec![],
                "complete",
                false,
            ))
        });
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some("complete".to_string())))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| {
            Ok(transaction_page(
                vec![purchase("transfer")],
                vec![],
                vec![],
                "replayed",
                false,
            ))
        });
    fixture.state.plaid_provider = Some(Arc::new(provider));

    // save the final cursor only after every page has been fetched; Plaid signs must be reversed
    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported":2}))
    );
    let rows: Vec<(String, Decimal)> = sqlx::query_as(
        "SELECT t.kind, t.amount FROM transactions t JOIN imported_transaction_details d ON d.transaction_id = t.id
         WHERE d.connection_id = $1 ORDER BY d.provider_transaction_id",
    ).bind(fixture.connection_id).fetch_all(&fixture.state.pool).await.unwrap();
    assert_eq!(
        rows,
        vec![
            ("income".to_string(), Decimal::new(12025, 2)),
            ("transfer".to_string(), Decimal::new(-1250, 2))
        ]
    );
    assert_eq!(
        fixture.saved_progress().await.0.as_deref(),
        Some("complete")
    );

    // the same provider identity updates its existing row rather than adding a second copy
    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported":0}))
    );
    assert_eq!(fixture.saved_progress().await.2, 2);
}

#[tokio::test]
async fn test_import_failure_leaves_transactions_and_saved_progress_untouched() {
    for failure in ["provider", "invalid-record", "stuck-cursor", "database"] {
        let mut fixture = ImportFixture::new().await;
        let mut provider = MockBankProvider::new();
        provider
            .expect_fetch_transactions()
            .with(eq(ACCESS_TOKEN), eq(None))
            .times(1)
            .returning(|_, _| {
                Ok(transaction_page(
                    vec![purchase("first")],
                    vec![],
                    vec![],
                    "page-one",
                    true,
                ))
            });
        provider
            .expect_fetch_transactions()
            .with(eq(ACCESS_TOKEN), eq(Some("page-one".to_string())))
            .times(1)
            .returning(move |_, _| match failure {
                "provider" => Err(PlaidError::RequestFailed),
                "stuck-cursor" => Ok(transaction_page(vec![], vec![], vec![], "page-one", true)),
                _ => {
                    let mut second = purchase("second");
                    second["amount"] = if failure == "database" {
                        json!(99999999999999.0)
                    } else {
                        json!(0)
                    };
                    Ok(transaction_page(
                        vec![second],
                        vec![],
                        vec![],
                        "complete",
                        false,
                    ))
                }
            });
        fixture.state.plaid_provider = Some(Arc::new(provider));

        // an upstream, parsing, or SQL failure must not leave page one saved or advance the cursor
        let (status, body) = fixture.import().await;
        assert_eq!(
            status,
            if failure == "database" {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::BAD_GATEWAY
            },
            "{failure}: {body}"
        );
        assert!(!body.to_string().contains(ACCESS_TOKEN));
        assert_eq!(fixture.saved_progress().await, (None, None, 0), "{failure}");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE user_id = $1")
            .bind(fixture.user_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{failure}");
    }
}

#[tokio::test]
async fn test_bank_corrections_preserve_each_user_override_and_initial_source_data() {
    let mut fixture = ImportFixture::new().await;
    let mut provider = MockBankProvider::new();
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .returning(|_, _| {
            Ok(transaction_page(
                (0..4).map(|i| purchase(&format!("edit-{i}"))).collect(),
                vec![],
                vec![],
                "initial",
                false,
            ))
        });
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some("initial".to_string())))
        .times(1)
        .returning(|_, _| {
            let modified = (0..4)
                .map(|i| {
                    let mut value = provider_transaction(
                        &format!("edit-{i}"),
                        -50.25,
                        "2026-10-02",
                        "Corrected bank name",
                    );
                    value["iso_currency_code"] = json!("USD");
                    value["account_id"] = json!("corrected-account");
                    value["personal_finance_category"] =
                        json!({"primary":"INCOME", "detailed":"INCOME_OTHER_INCOME"});
                    value
                })
                .collect();
            Ok(transaction_page(
                vec![],
                modified,
                vec![],
                "corrected",
                false,
            ))
        });
    fixture.state.plaid_provider = Some(Arc::new(provider));
    assert_eq!(fixture.import().await.0, StatusCode::OK);

    // simulate independent user choices: one kind edit, one cleared category, and one description edit
    for index in 0..4 {
        sqlx::query(
            "UPDATE transactions t SET kind_overridden = $2, category_overridden = $3, description_overridden = $4,
                    kind = CASE WHEN $2 THEN 'transfer' ELSE kind END,
                    category = CASE WHEN $3 THEN NULL ELSE category END,
                    description = CASE WHEN $4 THEN 'My description' ELSE description END
             FROM imported_transaction_details d WHERE d.transaction_id = t.id AND d.connection_id = $1 AND d.provider_transaction_id = $5",
        ).bind(fixture.connection_id).bind(index == 0).bind(index == 1).bind(index == 2).bind(format!("edit-{index}"))
            .execute(&fixture.state.pool).await.unwrap();
    }
    // seed stale search data so the correction must invalidate it when visible semantic fields change
    sqlx::query(
        "INSERT INTO transaction_embeddings (transaction_id, user_id, embedding_text, embedding, search_revision, embedding_model)
         SELECT t.id, t.user_id, 'test original description', $2, t.search_revision, 'test-model' FROM transactions t
         JOIN imported_transaction_details d ON d.transaction_id = t.id WHERE d.connection_id = $1",
    ).bind(fixture.connection_id).bind(pgvector::Vector::from(vec![0.0f32;1536])).execute(&fixture.state.pool).await.unwrap();

    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported":0}))
    );
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        Decimal,
        NaiveDate,
        Option<String>,
        i32,
        Value,
        String,
    )> = sqlx::query_as(
        "SELECT t.kind, t.category, t.description, t.amount, t.date, t.currency, t.search_revision,
                d.original_payload, d.provider_account_id
         FROM transactions t JOIN imported_transaction_details d ON d.transaction_id = t.id
         WHERE d.connection_id = $1 ORDER BY d.provider_transaction_id",
    )
    .bind(fixture.connection_id)
    .fetch_all(&fixture.state.pool)
    .await
    .unwrap();
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.0, if index == 0 { "transfer" } else { "income" });
        assert_eq!(
            row.1.as_deref(),
            if index == 1 {
                None
            } else {
                Some("INCOME_OTHER_INCOME")
            }
        );
        assert_eq!(
            row.2.as_deref(),
            Some(if index == 2 {
                "My description"
            } else {
                "Corrected bank name"
            })
        );
        assert_eq!(row.3, Decimal::new(5025, 2));
        assert_eq!(row.4.to_string(), "2026-10-02");
        assert_eq!(row.5.as_deref(), Some("USD"));
        assert_eq!(row.6, 2);
        assert_eq!(row.7, purchase(&format!("edit-{index}")));
        assert_eq!(row.8, "corrected-account");
    }
    let preserved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM imported_transaction_details WHERE connection_id = $1
         AND inferred_kind = 'expense' AND inferred_category = 'FOOD_AND_DRINK_GROCERIES'
         AND inferred_description = 'Original groceries' AND source_status = 'posted'",
    )
    .bind(fixture.connection_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(preserved, 4);
    let embeddings: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transaction_embeddings e JOIN imported_transaction_details d ON d.transaction_id = e.transaction_id WHERE d.connection_id = $1",
    ).bind(fixture.connection_id).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(embeddings, 0);
}

#[tokio::test]
async fn test_plaid_corrections_apply_after_revert_releases_user_overrides() {
    let mut fixture = ImportFixture::new().await;
    let mut provider = MockBankProvider::new();

    // The first call imports groceries; the next call corrects the same bank transaction.
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .returning(|_, _| {
            Ok(transaction_page(
                vec![purchase("reverted-transaction")],
                vec![],
                vec![],
                "initial",
                false,
            ))
        });
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some("initial".to_string())))
        .times(1)
        .returning(|_, _| {
            let mut corrected = provider_transaction(
                "reverted-transaction",
                -50.25,
                "2026-10-02",
                "Corrected bank name",
            );
            corrected["personal_finance_category"] =
                json!({"primary": "INCOME", "detailed": "INCOME_OTHER_INCOME"});
            Ok(transaction_page(
                vec![],
                vec![corrected],
                vec![],
                "corrected",
                false,
            ))
        });
    fixture.state.plaid_provider = Some(Arc::new(provider));
    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported": 1}))
    );
    let id: Uuid = sqlx::query_scalar(
        "SELECT transaction_id FROM imported_transaction_details WHERE connection_id = $1",
    )
    .bind(fixture.connection_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let source_before: Value = sqlx::query_scalar(
        "SELECT to_jsonb(d) FROM imported_transaction_details d WHERE transaction_id = $1",
    )
    .bind(id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();

    // A real edit sets all three override flags; Revert must release them before the next import.
    let app = build_app(fixture.state.clone());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/transactions/{id}"))
                .header("Authorization", format!("Bearer {}", fixture.jwt))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({
                        "expected_version": 1, "amount": "-20.00", "date": "2026-10-01", "kind": "Income",
                        "category": "My category", "description": "My purchase"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let flags: Vec<bool> = sqlx::query_scalar(
        "SELECT ARRAY[kind_overridden, category_overridden, description_overridden]
         FROM transactions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(flags, vec![true, true, true]);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/transactions/{id}/revert"))
                .header("Authorization", format!("Bearer {}", fixture.jwt))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Import must now accept all corrected fields, updating the existing row rather than duplicating it.
    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported": 0}))
    );
    let (kind, category, description, amount, date): (
        String,
        Option<String>,
        Option<String>,
        Decimal,
        NaiveDate,
    ) = sqlx::query_as(
        "SELECT kind, category, description, amount, date FROM transactions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(kind, "income");
    assert_eq!(category.as_deref(), Some("INCOME_OTHER_INCOME"));
    assert_eq!(description.as_deref(), Some("Corrected bank name"));
    assert_eq!(amount, Decimal::new(5025, 2));
    assert_eq!(date.to_string(), "2026-10-02");
    let (_, _, count) = fixture.saved_progress().await;
    assert_eq!(count, 1);

    // Bank corrections must still leave the initial payload and inferred values available for Revert.
    let source_after: Value = sqlx::query_scalar(
        "SELECT to_jsonb(d) FROM imported_transaction_details d WHERE transaction_id = $1",
    )
    .bind(id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(source_after, source_before);
}

#[tokio::test]
async fn test_posted_transaction_reuses_pending_row_and_removals_retain_history() {
    let mut fixture = ImportFixture::new().await;
    let mut provider = MockBankProvider::new();
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .returning(|_, _| {
            let mut pending = purchase("pending-id");
            pending["pending"] = json!(true);
            Ok(transaction_page(
                vec![pending, purchase("deleted-id")],
                vec![],
                vec![],
                "initial",
                false,
            ))
        });
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some("initial".to_string())))
        .times(1)
        .returning(|_, _| {
            let mut posted = purchase("posted-id");
            posted["pending_transaction_id"] = json!("pending-id");
            Ok(transaction_page(
                vec![posted],
                vec![],
                vec![
                    json!({"transaction_id":"pending-id"}),
                    json!({"transaction_id":"deleted-id"}),
                ],
                "posted",
                false,
            ))
        });
    fixture.state.plaid_provider = Some(Arc::new(provider));
    assert_eq!(fixture.import().await.0, StatusCode::OK);
    let original_id: Uuid = sqlx::query_scalar(
        "SELECT transaction_id FROM imported_transaction_details WHERE connection_id = $1 AND provider_transaction_id = 'pending-id'",
    ).bind(fixture.connection_id).fetch_one(&fixture.state.pool).await.unwrap();

    // an explicit pending ID links the posted version; the pending removal must not remove the posted row
    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported":0}))
    );
    let rows: Vec<(Uuid, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT t.id, d.provider_transaction_id, t.status, d.source_status, d.pending_transaction_id
         FROM transactions t JOIN imported_transaction_details d ON d.transaction_id = t.id
         WHERE d.connection_id = $1 ORDER BY d.provider_transaction_id",
    ).bind(fixture.connection_id).fetch_all(&fixture.state.pool).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (&rows[0].1, &rows[0].2),
        (&"deleted-id".to_string(), &"removed".to_string())
    );
    assert_eq!(
        rows[1],
        (
            original_id,
            "posted-id".to_string(),
            "posted".to_string(),
            "pending".to_string(),
            Some("pending-id".to_string())
        )
    );
}

// release both external calls together so both requests definitely fetched from the same saved cursor
struct ConcurrentProvider(tokio::sync::Barrier);

#[async_trait]
impl PlaidProvider for ConcurrentProvider {
    async fn create_link_token(&self, _: Uuid) -> Result<LinkToken, PlaidError> {
        panic!("unexpected Link request")
    }
    async fn exchange_public_token(&self, _: &str) -> Result<ExchangedConnection, PlaidError> {
        panic!("unexpected exchange")
    }
    async fn fetch_transactions(
        &self,
        access_token: &str,
        cursor: Option<String>,
    ) -> Result<TransactionPage, PlaidError> {
        assert_eq!(access_token, ACCESS_TOKEN);
        assert_eq!(cursor, None);
        self.0.wait().await;
        Ok(transaction_page(
            vec![purchase("concurrent-id")],
            vec![],
            vec![],
            "complete",
            false,
        ))
    }
}

#[tokio::test]
async fn test_overlapping_imports_cannot_save_the_same_batch_twice() {
    let mut fixture = ImportFixture::new().await;
    fixture.state.plaid_provider = Some(Arc::new(ConcurrentProvider(tokio::sync::Barrier::new(2))));

    // the timeout also detects holding a database lock across the external call
    let (first, second) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(fixture.import(), fixture.import())
    })
    .await
    .expect("imports must not hold a database lock while waiting for Plaid");
    let mut statuses = [first.0.as_u16(), second.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    let progress = fixture.saved_progress().await;
    assert_eq!(progress.0.as_deref(), Some("complete"));
    assert_eq!(progress.2, 1);
}

#[tokio::test]
async fn test_plaid_client_rejects_failed_and_incomplete_pages_without_exposing_credentials() {
    for template in [
        ResponseTemplate::new(500).set_body_string(ACCESS_TOKEN),
        ResponseTemplate::new(200).set_body_string("invalid JSON"),
        ResponseTemplate::new(200).set_body_json(json!({"added":[], "next_cursor":"incomplete"})),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/transactions/sync"))
            .and(body_json(json!({"access_token": ACCESS_TOKEN})))
            .respond_with(template)
            .expect(1)
            .mount(&server)
            .await;
        let client = PlaidClient::new(
            reqwest::Client::new(),
            &server.uri(),
            "client".to_string(),
            "secret".to_string(),
            vec!["CA".to_string()],
        );
        let error = client
            .fetch_transactions(ACCESS_TOKEN, None)
            .await
            .err()
            .expect("invalid page must fail");
        assert!(!format!("{error:?}").contains(ACCESS_TOKEN));
    }
}
#[tokio::test]
async fn test_import_accepts_empty_initial_response_before_bank_data_is_ready() {
    let mut fixture = ImportFixture::new().await;
    let mut provider = MockBankProvider::new();
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .returning(|_, _| Ok(transaction_page(vec![], vec![], vec![], "", false)));
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some(String::new())))
        .times(1)
        .returning(|_, _| {
            Ok(transaction_page(
                vec![purchase("ready")],
                vec![],
                vec![],
                "ready",
                false,
            ))
        });
    fixture.state.plaid_provider = Some(Arc::new(provider));

    // Plaid documents an empty cursor and empty arrays while the first bank pull is still pending
    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported":0}))
    );
    let progress = fixture.saved_progress().await;
    assert_eq!(progress.0.as_deref(), Some(""));
    assert!(progress.1.is_some());
    assert_eq!(progress.2, 0);

    // a later request can fetch and save the bank data without resetting or recreating the connection
    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported":1}))
    );
    assert_eq!(fixture.saved_progress().await.0.as_deref(), Some("ready"));
}

#[tokio::test]
async fn test_money_only_correction_keeps_current_search_data_and_handles_missing_category() {
    let mut fixture = ImportFixture::new().await;
    let mut initial = purchase("money-only");
    initial["amount"] = json!(-50.25);
    initial["personal_finance_category"] = Value::Null;
    initial["iso_currency_code"] = Value::Null;
    initial["unofficial_currency_code"] = json!("GBX");
    let mut corrected = initial.clone();
    corrected["amount"] = json!(-75.50);
    corrected["date"] = json!("2026-10-02");
    let mut provider = MockBankProvider::new();
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .returning(move |_, _| {
            Ok(transaction_page(
                vec![initial.clone()],
                vec![],
                vec![],
                "initial",
                false,
            ))
        });
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some("initial".to_string())))
        .times(1)
        .returning(move |_, _| {
            Ok(transaction_page(
                vec![],
                vec![corrected.clone()],
                vec![],
                "corrected",
                false,
            ))
        });
    fixture.state.plaid_provider = Some(Arc::new(provider));
    assert_eq!(fixture.import().await.0, StatusCode::OK);

    // a financial-only correction must leave the existing embedding and its matching revision intact
    sqlx::query(
        "INSERT INTO transaction_embeddings (transaction_id, user_id, embedding_text, embedding, search_revision, embedding_model)
         SELECT t.id, t.user_id, 'test income description', $2, t.search_revision, 'test-model' FROM transactions t
         JOIN imported_transaction_details d ON d.transaction_id = t.id WHERE d.connection_id = $1",
    ).bind(fixture.connection_id).bind(pgvector::Vector::from(vec![0.0f32;1536])).execute(&fixture.state.pool).await.unwrap();
    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported":0}))
    );
    let row: (Decimal, NaiveDate, String, Option<String>, Option<String>, i32, i32) = sqlx::query_as(
        "SELECT t.amount, t.date, t.kind, t.category, t.currency, t.search_revision, e.search_revision
         FROM transactions t JOIN imported_transaction_details d ON d.transaction_id = t.id
         JOIN transaction_embeddings e ON e.transaction_id = t.id WHERE d.connection_id = $1",
    ).bind(fixture.connection_id).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(
        row,
        (
            Decimal::new(7550, 2),
            NaiveDate::from_ymd_opt(2026, 10, 2).unwrap(),
            "income".to_string(),
            None,
            Some("GBX".to_string()),
            1,
            1
        )
    );
}
#[tokio::test]
async fn test_plaid_client_recognizes_login_required_without_exposing_response_details() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/transactions/sync"))
        .and(header("PLAID-CLIENT-ID", "test-client-id"))
        .and(header("PLAID-SECRET", "test-sandbox-secret"))
        .and(body_json(json!({"access_token": ACCESS_TOKEN})))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error_type": "ITEM_ERROR",
            "error_code": "ITEM_LOGIN_REQUIRED",
            "error_message": format!("private provider detail: {ACCESS_TOKEN}"),
            "display_message": "test-sandbox-secret",
            "request_id": "private-request-id"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = PlaidClient::new(
        reqwest::Client::new(),
        &server.uri(),
        "test-client-id".to_string(),
        "test-sandbox-secret".to_string(),
        vec!["US".to_string()],
    );

    // Keep only the reason the importer needs; upstream messages and credentials must not travel with it.
    let error = client
        .fetch_transactions(ACCESS_TOKEN, None)
        .await
        .err()
        .unwrap();
    assert_eq!(error, PlaidError::LoginRequired);
    for private_detail in [
        ACCESS_TOKEN,
        "test-sandbox-secret",
        "private-request-id",
        "private provider detail",
    ] {
        assert!(!format!("{error:?}").contains(private_detail));
    }
}

#[tokio::test]
async fn test_import_login_required_marks_only_requested_connection_and_preserves_saved_data() {
    let mut fixture = ImportFixture::new().await;
    let other = ImportFixture::new().await;

    // Include another bank for this owner as well as another owner's bank, to catch an overbroad status update.
    let sibling_provider_id = Uuid::new_v4().to_string();
    let sibling_credential = BankCredentialCipher::new(&TEST_KEY)
        .encrypt(
            ACCESS_TOKEN,
            &CredentialContext {
                user_id: fixture.user_id,
                provider: "plaid",
                provider_id: &sibling_provider_id,
            },
        )
        .unwrap();
    let sibling_id: Uuid = sqlx::query_scalar(
        "INSERT INTO bank_connections (user_id, provider, provider_id, encrypted_access_token)
         VALUES ($1, 'plaid', $2, $3) RETURNING connection_id",
    )
    .bind(fixture.user_id)
    .bind(sibling_provider_id)
    .bind(sibling_credential)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let other_connections: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(c) FROM bank_connections c WHERE connection_id = ANY($1) ORDER BY connection_id",
    ).bind(vec![sibling_id, other.connection_id]).fetch_all(&fixture.state.pool).await.unwrap();

    // Start with a completed import, so a failed later import must preserve real rows and a usable resume cursor.
    let mut provider = MockBankProvider::new();
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .returning(|_, _| {
            Ok(transaction_page(
                vec![purchase("saved")],
                vec![],
                vec![],
                "saved-progress",
                false,
            ))
        });
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some("saved-progress".to_string())))
        .times(1)
        .returning(|_, _| Err(PlaidError::LoginRequired));
    fixture.state.plaid_provider = Some(Arc::new(provider));
    assert_eq!(
        fixture.import().await,
        (StatusCode::OK, json!({"imported": 1}))
    );
    let before = fixture.saved_connection().await;
    let transactions = fixture.saved_transactions().await;

    // Return a bank-specific error without logging the user out of FinanceTracker or discarding saved bank data.
    assert_eq!(
        fixture.import().await,
        (
            StatusCode::CONFLICT,
            json!("Bank connection needs reconnection")
        )
    );
    let after = fixture.saved_connection().await;
    let mut expected = before.clone();
    expected["status"] = json!("needs_reconnection");
    expected["updated_at"] = after["updated_at"].clone();
    assert_eq!(after, expected);
    assert_eq!(fixture.saved_transactions().await, transactions);
    assert_eq!(
        fixture.saved_progress().await.0.as_deref(),
        Some("saved-progress")
    );
    let unchanged_connections: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(c) FROM bank_connections c WHERE connection_id = ANY($1) ORDER BY connection_id",
    ).bind(vec![sibling_id, other.connection_id]).fetch_all(&fixture.state.pool).await.unwrap();
    assert_eq!(unchanged_connections, other_connections);

    // The existing listing must expose the persisted status, and another import must stop before calling Plaid again.
    let request = Request::builder()
        .uri("/api/bank-connections")
        .header("Authorization", format!("Bearer {}", fixture.jwt))
        .body(Body::empty())
        .unwrap();
    let response = build_app(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let connections: Vec<Value> = serde_json::from_slice(&bytes).unwrap();
    let saved = connections
        .iter()
        .find(|c| c["connection_id"] == json!(fixture.connection_id))
        .unwrap();
    assert_eq!(saved["status"], "needs_reconnection");
    assert_eq!(fixture.import().await.0, StatusCode::CONFLICT);
    assert_eq!(fixture.saved_connection().await, after);
}

#[tokio::test]
async fn test_import_login_required_on_later_page_discards_incomplete_batch() {
    let mut fixture = ImportFixture::new().await;
    let mut provider = MockBankProvider::new();
    let mut sequence = mockall::Sequence::new();

    // Save the starting state normally; the next import will receive additions, changes, and a removal before failing.
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| {
            Ok(transaction_page(
                vec![purchase("saved")],
                vec![],
                vec![],
                "saved-progress",
                false,
            ))
        });
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some("saved-progress".to_string())))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| {
            let mut corrected = purchase("saved");
            corrected["name"] = json!("Bank correction from incomplete import");
            corrected["amount"] = json!(75.25);
            Ok(transaction_page(
                vec![purchase("unsaved")],
                vec![corrected],
                vec![json!({"transaction_id": "saved"})],
                "incomplete-page",
                true,
            ))
        });
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(Some("incomplete-page".to_string())))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Err(PlaidError::LoginRequired));
    fixture.state.plaid_provider = Some(Arc::new(provider));
    assert_eq!(fixture.import().await.0, StatusCode::OK);
    let progress = fixture.saved_progress().await;
    let transactions = fixture.saved_transactions().await;
    let mut expected = fixture.saved_connection().await;

    // Only the connection status may change: page one cannot add, edit, remove, or advance anything on its own.
    assert_eq!(
        fixture.import().await,
        (
            StatusCode::CONFLICT,
            json!("Bank connection needs reconnection")
        )
    );
    let after = fixture.saved_connection().await;
    expected["status"] = json!("needs_reconnection");
    expected["updated_at"] = after["updated_at"].clone();
    assert_eq!(after, expected);
    assert_eq!(fixture.saved_progress().await, progress);
    assert_eq!(fixture.saved_transactions().await, transactions);
}

#[tokio::test]
async fn test_import_outages_do_not_require_reconnection_or_change_saved_data() {
    let mut fixture = ImportFixture::new().await;
    let mut provider = MockBankProvider::new();
    provider
        .expect_fetch_transactions()
        .with(eq(ACCESS_TOKEN), eq(None))
        .times(1)
        .returning(|_, _| {
            Ok(transaction_page(
                vec![purchase("saved")],
                vec![],
                vec![],
                "saved-progress",
                false,
            ))
        });
    fixture.state.plaid_provider = Some(Arc::new(provider));
    assert_eq!(fixture.import().await.0, StatusCode::OK);
    let connection = fixture.saved_connection().await;
    let transactions = fixture.saved_transactions().await;

    // A server outage, unrelated error, malformed response, or timeout must not be treated as a bank login request.
    for template in [
        ResponseTemplate::new(503).set_body_json(
            json!({"error_code": "INTERNAL_SERVER_ERROR", "error_message": ACCESS_TOKEN}),
        ),
        ResponseTemplate::new(400)
            .set_body_json(json!({"error_code": "ITEM_NOT_FOUND", "error_message": ACCESS_TOKEN})),
        ResponseTemplate::new(400).set_body_string(ACCESS_TOKEN),
        ResponseTemplate::new(200).set_body_json(json!({"added": []})),
        ResponseTemplate::new(200).set_delay(Duration::from_millis(250)),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/transactions/sync"))
            .and(body_json(
                json!({"access_token": ACCESS_TOKEN, "cursor": "saved-progress"}),
            ))
            .respond_with(template)
            .expect(1)
            .mount(&server)
            .await;
        fixture.state.plaid_provider = Some(Arc::new(PlaidClient::new(
            reqwest::Client::builder()
                .timeout(Duration::from_millis(100))
                .build()
                .unwrap(),
            &server.uri(),
            "client".to_string(),
            "secret".to_string(),
            vec!["US".to_string()],
        )));

        // Keep the error safe and retryable, including the last successful sync timestamp and encrypted credential.
        assert_eq!(
            fixture.import().await,
            (
                StatusCode::BAD_GATEWAY,
                json!("Could not fetch bank transactions")
            )
        );
        assert_eq!(fixture.saved_connection().await, connection);
        assert_eq!(fixture.saved_transactions().await, transactions);
    }
}
