mod common;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use financetracker::bank_credentials::{BankCredentialCipher, CredentialContext};
use financetracker::plaid::{
    ExchangedConnection, LinkToken, PlaidError, PlaidProvider, TransactionPage,
};
use financetracker::{AppState, build_app};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use mockall::mock;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::util::ServiceExt;
use uuid::Uuid;

// A listing reads our database; any attempt to create, exchange, or import through Plaid is a test failure.
mock! {
    BankProvider {}

    #[async_trait]
    impl PlaidProvider for BankProvider {
        async fn create_link_token(&self, user_id: Uuid) -> Result<LinkToken, PlaidError>;
        async fn exchange_public_token(&self, public_token: &str) -> Result<ExchangedConnection, PlaidError>;
        async fn fetch_transactions(&self, access_token: &str, cursor: Option<String>) -> Result<TransactionPage, PlaidError>;
    }
}

// Register and log in normally so ownership comes from the same JWT extractor used by the app.
async fn authenticated_state() -> (AppState, Uuid, String) {
    let state = common::setup_app_state().await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, jwt) = common::login_test_user(&app, &username, &password).await;
    (state, user_id, jwt)
}

// Store a real encrypted credential and progress data, including fields the listing must never return.
// A disconnected connection may have its credential removed while its history remains saved.
async fn save_connection(state: &AppState, user_id: Uuid, status: &str) -> Uuid {
    let provider_id = Uuid::new_v4().to_string();
    let encrypted = if status == "disconnected" {
        None
    } else {
        Some(
            BankCredentialCipher::new(&[41; 32])
                .encrypt(
                    "access-sandbox-listing-test-secret",
                    &CredentialContext {
                        user_id,
                        provider: "plaid",
                        provider_id: &provider_id,
                    },
                )
                .unwrap(),
        )
    };
    sqlx::query_scalar(
        "INSERT INTO bank_connections
         (user_id, provider, provider_id, encrypted_access_token, progress_cursor, status, last_synced_at)
         VALUES ($1, 'plaid', $2, $3, 'private-import-progress', $4, CURRENT_TIMESTAMP)
         RETURNING connection_id",
    )
    .bind(user_id)
    .bind(provider_id)
    .bind(encrypted)
    .bind(status)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

fn list_request(jwt: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method("GET")
        .uri("/api/bank-connections");
    if let Some(jwt) = jwt {
        request = request.header("Authorization", format!("Bearer {jwt}"));
    }
    request.body(Body::empty()).unwrap()
}

async fn json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("listing must return JSON")
}

#[tokio::test]
async fn test_list_returns_only_owned_connections_with_saved_statuses() {
    let (state, user_id, jwt) = authenticated_state().await;
    let app = build_app(state.clone());

    // Include every saved status: listing must not silently drop connections that need attention.
    let mut expected = Vec::new();
    for status in ["active", "needs_reconnection", "disconnected"] {
        let connection_id = save_connection(&state, user_id, status).await;
        expected
            .push(json!({"connection_id": connection_id, "provider": "plaid", "status": status}));
    }

    // Another user's connection must stay private even though it is in the same database.
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (other_user, _) = common::login_test_user(&app, &username, &password).await;
    save_connection(&state, other_user, "active").await;
    let response = app.oneshot(list_request(Some(&jwt))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Compare membership and saved statuses without imposing an ordering on the API.
    let mut actual = json(response).await.as_array().unwrap().clone();
    actual.sort_by_key(|row| row["connection_id"].as_str().unwrap().to_string());
    expected.sort_by_key(|row| row["connection_id"].as_str().unwrap().to_string());
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn test_list_returns_empty_array_for_user_without_connections() {
    let (state, _, jwt) = authenticated_state().await;
    let app = build_app(state.clone());

    // The database is not empty; only this user's list should be empty.
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (other_user, _) = common::login_test_user(&app, &username, &password).await;
    save_connection(&state, other_user, "active").await;
    let response = app.oneshot(list_request(Some(&jwt))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await, json!([]));
}

#[tokio::test]
async fn test_list_rejects_missing_invalid_and_expired_login_tokens() {
    let (state, user_id, _) = authenticated_state().await;

    // Sign an otherwise valid token far enough in the past to exceed the JWT expiration leeway.
    let expired = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &json!({"sub": user_id, "exp": (Utc::now() - Duration::hours(1)).timestamp()}),
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .unwrap();
    let app = build_app(state);
    let mut statuses = Vec::new();
    for jwt in [None, Some("invalid-jwt"), Some(expired.as_str())] {
        let response = app.clone().oneshot(list_request(jwt)).await.unwrap();
        statuses.push(response.status());
    }
    assert_eq!(statuses, vec![StatusCode::UNAUTHORIZED; 3]);
}

#[tokio::test]
async fn test_list_returns_only_safe_metadata_without_calling_plaid() {
    let (mut state, user_id, jwt) = authenticated_state().await;
    let connection_id = save_connection(&state, user_id, "active").await;
    let before: Value =
        sqlx::query_scalar("SELECT to_jsonb(c) FROM bank_connections c WHERE connection_id = $1")
            .bind(connection_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();

    // Listing must work without an encryption key and must not contact even a configured provider.
    let mut provider = MockBankProvider::new();
    provider.expect_create_link_token().times(0);
    provider.expect_exchange_public_token().times(0);
    provider.expect_fetch_transactions().times(0);
    state.plaid_provider = Some(Arc::new(provider));
    assert!(state.bank_credential_cipher.is_none());
    let response = build_app(state.clone())
        .oneshot(list_request(Some(&jwt)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Exact JSON equality rejects credentials, ciphertext, provider IDs, cursors, or any other extra field.
    assert_eq!(
        json(response).await,
        json!([{
            "connection_id": connection_id, "provider": "plaid", "status": "active"
        }])
    );

    // Reading connections must not consume progress or alter any stored connection data.
    let after: Value =
        sqlx::query_scalar("SELECT to_jsonb(c) FROM bank_connections c WHERE connection_id = $1")
            .bind(connection_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(after, before);
}
