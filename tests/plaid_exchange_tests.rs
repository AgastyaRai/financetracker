mod common;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use financetracker::bank_credentials::{BankCredentialCipher, CredentialContext};
use financetracker::plaid::TransactionPage;
use financetracker::plaid::{
    ExchangedConnection, LinkToken, PlaidClient, PlaidError, PlaidProvider,
};
use financetracker::{AppState, build_app};
use http_body_util::BodyExt;
use mockall::{mock, predicate::eq};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tower::util::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_KEY: [u8; 32] = [29; 32];
const PUBLIC_TOKEN: &str = "public-sandbox-exchange-test-token";
const ACCESS_TOKEN: &str = "access-sandbox-exchange-test-token";
const EXCHANGE_FAILURE: &str = "Could not complete bank linking";

// keep provider behavior controllable without adding mocking dependencies to application code
mock! {
    BankProvider {}

    #[async_trait]
    impl PlaidProvider for BankProvider {
        async fn create_link_token(&self, user_id: Uuid) -> Result<LinkToken, PlaidError>;
        async fn exchange_public_token(&self, public_token: &str) -> Result<ExchangedConnection, PlaidError>;
        async fn fetch_transactions(&self, access_token: &str, cursor: Option<String>) -> Result<TransactionPage, PlaidError>;
    }
}

// inspect the actual stored row, including fields that exchange must leave untouched on conflicts
#[derive(Debug, PartialEq, sqlx::FromRow)]
struct StoredConnection {
    connection_id: Uuid,
    user_id: Uuid,
    provider: String,
    provider_id: String,
    encrypted_access_token: Option<Vec<u8>>,
    progress_cursor: Option<String>,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_synced_at: Option<DateTime<Utc>>,
}

// use the real registration/login flow and inject only a fixed test encryption key
async fn authenticated_state() -> (AppState, Uuid, String) {
    let mut state = common::setup_app_state().await;
    state.bank_credential_cipher = Some(Arc::new(BankCredentialCipher::new(&TEST_KEY)));
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, jwt) = common::login_test_user(&app, &username, &password).await;
    (state, user_id, jwt)
}

fn exchange_request(jwt: Option<&str>, body: Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/bank-connections/exchange-token")
        .header("Content-Type", "application/json");
    if let Some(jwt) = jwt {
        request = request.header("Authorization", format!("Bearer {jwt}"));
    }
    request.body(Body::from(body.to_string())).unwrap()
}

fn exchanged_connection(provider_id: &str) -> ExchangedConnection {
    ExchangedConnection {
        provider_id: provider_id.to_string(),
        access_token: ACCESS_TOKEN.to_string(),
    }
}

// success requires one exchange of the submitted token, with no automatic retries
fn successful_provider(provider_id: &str) -> MockBankProvider {
    let provider_id = provider_id.to_string();
    let mut provider = MockBankProvider::new();
    provider.expect_create_link_token().times(0);
    provider
        .expect_exchange_public_token()
        .with(eq(PUBLIC_TOKEN))
        .times(1)
        .returning(move |_| Ok(exchanged_connection(&provider_id)));
    provider
}

async fn stored_connection(state: &AppState, provider_id: &str) -> StoredConnection {
    sqlx::query_as::<_, StoredConnection>(
        "SELECT connection_id, user_id, provider, provider_id, encrypted_access_token,
                progress_cursor, status, created_at, updated_at, last_synced_at
         FROM bank_connections WHERE provider = 'plaid' AND provider_id = $1",
    )
    .bind(provider_id)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

async fn connection_count(state: &AppState, user_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM bank_connections WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(&state.pool)
        .await
        .unwrap()
}

// real HTTP-client tests may contact only this local mock server
fn local_client(server: &MockServer, timeout: Duration) -> PlaidClient {
    PlaidClient::new(
        reqwest::Client::builder().timeout(timeout).build().unwrap(),
        &server.uri(),
        "test-client-id".to_string(),
        "test-sandbox-secret".to_string(),
        vec!["US".to_string(), "CA".to_string()],
    )
}

// a completed exchange saves an encrypted credential for the JWT owner and returns only safe metadata
#[tokio::test]
async fn test_exchange_saves_encrypted_connection_for_authenticated_user() {
    let (mut state, user_id, jwt) = authenticated_state().await;
    let provider_id = Uuid::new_v4().to_string();
    state.plaid_provider = Some(Arc::new(successful_provider(&provider_id)));

    // client-supplied ownership, provider identity, credentials, and progress cannot select stored values
    let response = build_app(state.clone())
        .oneshot(exchange_request(
            Some(&jwt),
            json!({
                "public_token": PUBLIC_TOKEN,
                "user_id": Uuid::new_v4(),
                "provider": "client-provider",
                "provider_id": "client-connection",
                "access_token": "client-credential",
                "status": "disconnected",
                "progress_cursor": "client-progress",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let response: Value = serde_json::from_slice(&body).unwrap();
    let stored = stored_connection(&state, &provider_id).await;
    assert_eq!(
        response,
        json!({
            "connection_id": stored.connection_id,
            "provider": "plaid",
            "status": "active",
        })
    );

    // exchange establishes a connection; successful import progress must wait for an actual sync
    assert_eq!(stored.user_id, user_id);
    assert_eq!(stored.provider, "plaid");
    assert_eq!(stored.provider_id, provider_id);
    assert_eq!(stored.status, "active");
    assert!(stored.progress_cursor.is_none());
    assert!(stored.last_synced_at.is_none());
    assert_eq!(connection_count(&state, user_id).await, 1);

    // prove the database holds usable ciphertext bound to this owner and provider connection
    let encrypted = stored.encrypted_access_token.unwrap();
    assert_ne!(encrypted.as_slice(), ACCESS_TOKEN.as_bytes());
    let context = CredentialContext {
        user_id,
        provider: "plaid",
        provider_id: &provider_id,
    };
    assert_eq!(
        BankCredentialCipher::new(&TEST_KEY)
            .decrypt(&encrypted, &context)
            .unwrap(),
        ACCESS_TOKEN,
    );
}

// authentication must reject absent, malformed, and expired JWTs before consuming a public token
#[tokio::test]
async fn test_exchange_rejects_invalid_authentication_without_provider_calls() {
    let (mut state, user_id, _) = authenticated_state().await;
    let mut provider = MockBankProvider::new();
    provider.expect_create_link_token().times(0);
    provider.expect_exchange_public_token().times(0);
    state.plaid_provider = Some(Arc::new(provider));
    let expired_jwt = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({"sub": user_id, "exp": (Utc::now() - chrono::Duration::minutes(5)).timestamp()}),
        &jsonwebtoken::EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .unwrap();
    let app = build_app(state.clone());

    for jwt in [None, Some("invalid-jwt"), Some(expired_jwt.as_str())] {
        let response = app
            .clone()
            .oneshot(exchange_request(jwt, json!({"public_token": PUBLIC_TOKEN})))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(connection_count(&state, user_id).await, 0);
}

// reject empty input locally rather than make a provider request that cannot complete linking
#[tokio::test]
async fn test_exchange_rejects_blank_public_token_without_provider_calls() {
    let (mut state, user_id, jwt) = authenticated_state().await;
    let mut provider = MockBankProvider::new();
    provider.expect_exchange_public_token().times(0);
    state.plaid_provider = Some(Arc::new(provider));
    let app = build_app(state.clone());

    for public_token in ["", " ", "\t\r\n"] {
        let response = app
            .clone()
            .oneshot(exchange_request(
                Some(&jwt),
                json!({"public_token": public_token}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(connection_count(&state, user_id).await, 0);
}

// both provider access and encryption must be ready before the backend consumes a one-use token
#[tokio::test]
async fn test_exchange_requires_provider_and_cipher_without_provider_calls() {
    let (state, user_id, jwt) = authenticated_state().await;
    for missing_provider in [true, false] {
        let mut state = state.clone();
        if missing_provider {
            state.plaid_provider = None;
        } else {
            let mut provider = MockBankProvider::new();
            provider.expect_exchange_public_token().times(0);
            state.plaid_provider = Some(Arc::new(provider));
            state.bank_credential_cipher = None;
        }
        let response = build_app(state)
            .oneshot(exchange_request(
                Some(&jwt),
                json!({"public_token": PUBLIC_TOKEN}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
    assert_eq!(connection_count(&state, user_id).await, 0);
}

// provider failures return a controlled error and leave no saved connection
#[tokio::test]
async fn test_exchange_provider_failure_returns_sanitized_bad_gateway_without_saving() {
    let (mut state, user_id, jwt) = authenticated_state().await;
    let mut provider = MockBankProvider::new();
    provider
        .expect_exchange_public_token()
        .with(eq(PUBLIC_TOKEN))
        .times(1)
        .returning(|_| Err(PlaidError::RequestFailed));
    state.plaid_provider = Some(Arc::new(provider));
    let response = build_app(state.clone())
        .oneshot(exchange_request(
            Some(&jwt),
            json!({"public_token": PUBLIC_TOKEN}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(std::str::from_utf8(&body).unwrap(), EXCHANGE_FAILURE);
    assert_eq!(connection_count(&state, user_id).await, 0);
}

// an existing connection must survive a repeated exchange attempt, including one from another owner
#[tokio::test]
async fn test_exchange_does_not_overwrite_existing_connection() {
    let (state, user_id, jwt) = authenticated_state().await;
    let (_, other_user_id, _) = authenticated_state().await;
    for owner_id in [user_id, other_user_id] {
        let provider_id = Uuid::new_v4().to_string();
        let context = CredentialContext {
            user_id: owner_id,
            provider: "plaid",
            provider_id: &provider_id,
        };
        let encrypted = BankCredentialCipher::new(&TEST_KEY)
            .encrypt("previous-access-token", &context)
            .unwrap();

        // seed meaningful import progress so an accidental upsert would damage more than ownership
        sqlx::query(
            "INSERT INTO bank_connections
             (user_id, provider, provider_id, encrypted_access_token, status, progress_cursor, last_synced_at)
             VALUES ($1, 'plaid', $2, $3, 'needs_reconnection', 'saved-cursor', CURRENT_TIMESTAMP)",
        )
        .bind(owner_id).bind(&provider_id).bind(encrypted)
        .execute(&state.pool).await.unwrap();
        let before = stored_connection(&state, &provider_id).await;
        let mut state = state.clone();
        state.plaid_provider = Some(Arc::new(successful_provider(&provider_id)));
        let response = build_app(state.clone())
            .oneshot(exchange_request(
                Some(&jwt),
                json!({"public_token": PUBLIC_TOKEN}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(stored_connection(&state, &provider_id).await, before);
    }
}

// the database uniqueness rule must also protect overlapping requests, not only sequential repeats
#[tokio::test]
async fn test_concurrent_exchange_saves_only_one_connection() {
    let (mut state, user_id, jwt) = authenticated_state().await;
    let provider_id = Uuid::new_v4().to_string();
    let result_id = provider_id.clone();
    let mut provider = MockBankProvider::new();
    provider
        .expect_exchange_public_token()
        .withf(|token| matches!(token, "public-concurrent-a" | "public-concurrent-b"))
        .times(2)
        .returning(move |_| Ok(exchanged_connection(&result_id)));
    state.plaid_provider = Some(Arc::new(provider));
    let app = build_app(state.clone());

    // use distinct public tokens for the same provider identity; only one insert may commit
    let (first, second) = tokio::join!(
        app.clone().oneshot(exchange_request(
            Some(&jwt),
            json!({"public_token": "public-concurrent-a"})
        )),
        app.oneshot(exchange_request(
            Some(&jwt),
            json!({"public_token": "public-concurrent-b"})
        )),
    );
    let mut statuses = [first.unwrap().status(), second.unwrap().status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);
    assert_eq!(connection_count(&state, user_id).await, 1);
    assert_eq!(
        stored_connection(&state, &provider_id).await.user_id,
        user_id
    );
}

// exchange success alone is insufficient: a failed database save must not report a saved connection
#[tokio::test]
async fn test_exchange_database_failure_does_not_report_success_or_retry() {
    let (mut state, user_id, jwt) = authenticated_state().await;
    let observer = common::setup_app_state().await;
    state.plaid_provider = Some(Arc::new(successful_provider(&Uuid::new_v4().to_string())));

    // close only this test's pool to fail the insert without altering shared database roles or schema
    state.pool.close().await;
    let response = build_app(state)
        .oneshot(exchange_request(
            Some(&jwt),
            json!({"public_token": PUBLIC_TOKEN}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        std::str::from_utf8(&body).unwrap(),
        "Could not save bank connection"
    );
    assert_eq!(connection_count(&observer, user_id).await, 0);
}

// exercise the real client against the exact Plaid exchange request and response contract
#[tokio::test]
async fn test_plaid_client_exchanges_public_token_and_maps_provider_identity() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/item/public_token/exchange"))
        .and(header("PLAID-CLIENT-ID", "test-client-id"))
        .and(header("PLAID-SECRET", "test-sandbox-secret"))
        .and(body_json(json!({"public_token": PUBLIC_TOKEN})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": ACCESS_TOKEN,
            "item_id": "provider-exchange-test-connection",
            "request_id": "private-request-id",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // the provider's item_id is kept as provider_id in our domain and stored connection
    let result = local_client(&server, Duration::from_secs(5))
        .exchange_public_token(PUBLIC_TOKEN)
        .await
        .unwrap();
    assert_eq!(result.provider_id, "provider-exchange-test-connection");
    assert_eq!(result.access_token, ACCESS_TOKEN);
}

// upstream statuses and raw response bodies must not leak through the exchange endpoint
#[tokio::test]
async fn test_exchange_http_errors_return_sanitized_bad_gateway_without_saving() {
    let (state, user_id, jwt) = authenticated_state().await;
    for status in [401, 429, 500] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/item/public_token/exchange"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({
                "error_message": "private failure containing test-sandbox-secret",
                "access_token": ACCESS_TOKEN,
                "public_token": PUBLIC_TOKEN,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut state = state.clone();
        state.plaid_provider = Some(Arc::new(local_client(&server, Duration::from_secs(5))));
        let response = build_app(state)
            .oneshot(exchange_request(
                Some(&jwt),
                json!({"public_token": PUBLIC_TOKEN}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(std::str::from_utf8(&body).unwrap(), EXCHANGE_FAILURE);
    }
    assert_eq!(connection_count(&state, user_id).await, 0);
}

// successful HTTP status must still provide both a usable credential and a provider connection ID
#[tokio::test]
async fn test_exchange_rejects_malformed_or_incomplete_provider_responses() {
    let (state, user_id, jwt) = authenticated_state().await;
    for body in [
        "not json".to_string(),
        json!({"item_id": "provider-connection"}).to_string(),
        json!({"access_token": ACCESS_TOKEN}).to_string(),
        json!({"access_token": "", "item_id": "provider-connection"}).to_string(),
        json!({"access_token": " \t", "item_id": "provider-connection"}).to_string(),
        json!({"access_token": ACCESS_TOKEN, "item_id": ""}).to_string(),
        json!({"access_token": ACCESS_TOKEN, "item_id": " \t"}).to_string(),
        json!({"access_token": ACCESS_TOKEN, "item_id": false}).to_string(),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/item/public_token/exchange"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        let mut state = state.clone();
        state.plaid_provider = Some(Arc::new(local_client(&server, Duration::from_secs(5))));
        let response = build_app(state)
            .oneshot(exchange_request(
                Some(&jwt),
                json!({"public_token": PUBLIC_TOKEN}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(std::str::from_utf8(&body).unwrap(), EXCHANGE_FAILURE);
    }
    assert_eq!(connection_count(&state, user_id).await, 0);
}

// a delayed response must time out once, leaving no row and no credential-bearing error text
#[tokio::test]
async fn test_exchange_timeout_returns_sanitized_bad_gateway_without_saving() {
    let (mut state, user_id, jwt) = authenticated_state().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/item/public_token/exchange"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_json(
                    json!({"access_token": ACCESS_TOKEN, "item_id": "slow-provider-connection"}),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    state.plaid_provider = Some(Arc::new(local_client(&server, Duration::from_millis(250))));
    let response = build_app(state.clone())
        .oneshot(exchange_request(
            Some(&jwt),
            json!({"public_token": PUBLIC_TOKEN}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(std::str::from_utf8(&body).unwrap(), EXCHANGE_FAILURE);
    assert_eq!(connection_count(&state, user_id).await, 0);
}
