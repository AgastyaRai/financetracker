mod common;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use financetracker::plaid::TransactionPage;
use financetracker::plaid::{ExchangedConnection, LinkToken, PlaidClient, PlaidError, PlaidProvider};
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

// generate the mock in this integration test so Mockall stays a test dependency
mock! {
    BankProvider {}

    #[async_trait]
    impl PlaidProvider for BankProvider {
        async fn create_link_token(&self, user_id: Uuid) -> Result<LinkToken, PlaidError>;
        async fn exchange_public_token(&self, public_token: &str) -> Result<ExchangedConnection, PlaidError>;
        async fn fetch_transactions(&self, access_token: &str, cursor: Option<String>) -> Result<TransactionPage, PlaidError>;
    }
}

fn link_token() -> LinkToken {
    LinkToken {
        link_token: "link-sandbox-test-token".to_string(),
        expiration: "2030-01-01T04:00:00Z".parse().unwrap(),
    }
}

// register and log in normally; the linking tests use the app's actual JWT extractor
async fn authenticated_state() -> (AppState, Uuid, String) {
    let state = common::setup_app_state().await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;
    (state, user_id, access_token)
}

fn link_request(access_token: Option<&str>, body: Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/bank-connections/link-token")
        .header("Content-Type", "application/json");
    if let Some(access_token) = access_token {
        request = request.header("Authorization", format!("Bearer {}", access_token));
    }
    request.body(Body::from(body.to_string())).unwrap()
}

// localhost is the only outbound destination in HTTP-client tests
fn local_client(server: &MockServer, timeout: Duration) -> PlaidClient {
    let http_client = reqwest::Client::builder().timeout(timeout).build().unwrap();
    PlaidClient::new(
        http_client,
        &server.uri(),
        "test-client-id".to_string(),
        "test-sandbox-secret".to_string(),
        vec!["US".to_string(), "CA".to_string()],
    )
}

// the authenticated user gets the token, without creating a connection before exchange
#[tokio::test]
async fn test_link_token_uses_authenticated_user_without_saving_connection() {
    let (mut state, user_id, access_token) = authenticated_state().await;

    // require exactly one call with the UUID from login, then return a predictable token
    let mut provider = MockBankProvider::new();
    provider.expect_create_link_token()
        .with(eq(user_id))
        .times(1)
        .returning(|_| Ok(link_token()));
    state.plaid_provider = Some(Arc::new(provider));
    let app = build_app(state.clone());

    let response = app.oneshot(link_request(Some(&access_token), json!({}))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let response: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response, json!({
        "link_token": "link-sandbox-test-token",
        "expiration": "2030-01-01T04:00:00Z",
    }));

    // a startup token is not a completed bank connection and must not insert a placeholder
    let connections: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM bank_connections WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(connections, 0);
}

// missing, malformed, and expired credentials must all fail before touching Plaid
#[tokio::test]
async fn test_link_token_rejects_invalid_authentication_without_provider_calls() {
    let mut state = common::setup_app_state().await;
    let mut provider = MockBankProvider::new();
    provider.expect_create_link_token().times(0);
    state.plaid_provider = Some(Arc::new(provider));

    // expire well beyond the JWT library's clock-skew allowance
    let expired_token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({
            "sub": Uuid::new_v4().to_string(),
            "exp": (chrono::Utc::now() - chrono::Duration::minutes(5)).timestamp(),
        }),
        &jsonwebtoken::EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    ).unwrap();
    let app = build_app(state);

    for access_token in [None, Some("invalid-token"), Some(expired_token.as_str())] {
        let response = app.clone()
            .oneshot(link_request(access_token, json!({})))
            .await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

// a request body cannot choose another user or override server-owned Link settings
#[tokio::test]
async fn test_link_token_ignores_client_supplied_user_id() {
    let (mut state, user_id, access_token) = authenticated_state().await;
    let mut provider = MockBankProvider::new();
    provider.expect_create_link_token()
        .with(eq(user_id))
        .times(1)
        .returning(|_| Ok(link_token()));
    state.plaid_provider = Some(Arc::new(provider));

    // submit a different UUID; only the verified JWT may select the provider's user
    let response = build_app(state).oneshot(link_request(Some(&access_token), json!({
        "user_id": Uuid::new_v4(),
        "country_codes": ["GB"],
        "products": ["auth"],
    }))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

// leaving Plaid unconfigured keeps registration/login usable and reports linking unavailable
#[tokio::test]
async fn test_link_token_without_configuration_returns_service_unavailable() {
    let (state, _, access_token) = authenticated_state().await;
    assert!(state.plaid_provider.is_none());

    let response = build_app(state)
        .oneshot(link_request(Some(&access_token), json!({})))
        .await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// a provider failure is an upstream failure, not an invalid FinanceTracker login
#[tokio::test]
async fn test_link_token_provider_failure_returns_bad_gateway() {
    let (mut state, user_id, access_token) = authenticated_state().await;
    let mut provider = MockBankProvider::new();
    provider.expect_create_link_token()
        .with(eq(user_id))
        .times(1)
        .returning(|_| Err(PlaidError::RequestFailed));
    state.plaid_provider = Some(Arc::new(provider));

    let response = build_app(state)
        .oneshot(link_request(Some(&access_token), json!({})))
        .await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

// exercise the real reqwest client against a local server with an exact request contract
#[tokio::test]
async fn test_plaid_client_sends_transactions_link_request_and_parses_response() {
    let server = MockServer::start().await;
    let user_id = Uuid::new_v4();

    // credentials stay in the backend request; countries/products come from server settings
    Mock::given(method("POST"))
        .and(path("/link/token/create"))
        .and(header("PLAID-CLIENT-ID", "test-client-id"))
        .and(header("PLAID-SECRET", "test-sandbox-secret"))
        .and(body_json(json!({
            "user": { "client_user_id": user_id.to_string() },
            "client_name": "FinanceTracker",
            "products": ["transactions"],
            "country_codes": ["US", "CA"],
            "language": "en",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "link_token": "link-sandbox-test-token",
            "expiration": "2030-01-01T04:00:00Z",
            "request_id": "provider-request-id",
        })))
        .expect(1)
        .mount(&server).await;

    // unknown provider fields are excluded from the value exposed to the frontend
    let result = local_client(&server, Duration::from_secs(5))
        .create_link_token(user_id).await.unwrap();
    assert_eq!(result, link_token());
    assert_eq!(serde_json::to_value(result).unwrap(), json!({
        "link_token": "link-sandbox-test-token",
        "expiration": "2030-01-01T04:00:00Z",
    }));
}

// upstream status codes and raw error bodies must not escape through our authenticated route
#[tokio::test]
async fn test_plaid_http_errors_return_sanitized_bad_gateway() {
    let (state, _, access_token) = authenticated_state().await;

    for status in [401, 429, 500] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/link/token/create"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({
                "error_message": "provider failure containing test-sandbox-secret",
                "request_id": "private-provider-request-id",
            })))
            .expect(1)
            .mount(&server).await;

        // route through the real client so this checks status translation and sanitization together
        let mut state = state.clone();
        state.plaid_provider = Some(Arc::new(local_client(&server, Duration::from_secs(5))));
        let response = build_app(state)
            .oneshot(link_request(Some(&access_token), json!({})))
            .await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "upstream status {status}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "Could not create a bank-linking token");
    }
}

// successful HTTP status alone is insufficient: both usable response fields are required
#[tokio::test]
async fn test_plaid_client_rejects_malformed_or_incomplete_responses() {
    for body in [
        "not json".to_string(),
        json!({"expiration": "2030-01-01T04:00:00Z"}).to_string(),
        json!({"link_token": "link-sandbox-test-token"}).to_string(),
        json!({"link_token": "", "expiration": "2030-01-01T04:00:00Z"}).to_string(),
        json!({"link_token": "link-sandbox-test-token", "expiration": "invalid-date"}).to_string(),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/link/token/create"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .expect(1)
            .mount(&server).await;

        let result = local_client(&server, Duration::from_secs(5))
            .create_link_token(Uuid::new_v4()).await;
        assert!(result.is_err());
    }
}

// a slow upstream must stop at the configured HTTP timeout and produce the same controlled error
#[tokio::test]
async fn test_plaid_timeout_returns_sanitized_bad_gateway() {
    let (mut state, _, access_token) = authenticated_state().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/link/token/create"))
        .respond_with(ResponseTemplate::new(200)
            .set_delay(Duration::from_secs(2))
            .set_body_json(serde_json::to_value(link_token()).unwrap()))
        .expect(1)
        .mount(&server).await;
    state.plaid_provider = Some(Arc::new(local_client(&server, Duration::from_millis(250))));

    let response = build_app(state)
        .oneshot(link_request(Some(&access_token), json!({})))
        .await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(std::str::from_utf8(&body).unwrap(), "Could not create a bank-linking token");
}
