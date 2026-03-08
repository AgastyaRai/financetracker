use financetracker::AppState;
use http_body_util::BodyExt;
use tower::util::ServiceExt;

// helper function to create and register a unique test user and returns the username and password
pub async fn create_and_register_test_user(app: &axum::Router) -> (String, String) {
    // we use a unique suffix to ensure we can run tests repeatedly without conflicts
    let unique_suffix = uuid::Uuid::new_v4().to_string();

    let username = format!("testuser_{}", unique_suffix);
    let email = format!("{}@example.com", username);
    let password = "bestPassword";

    // register the user
    let register_body = serde_json::json!({
        "username": username,
        "email": email,
        "password": password,
    });

    let register_request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/users/register")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(register_body.to_string()))
        .unwrap();

    let register_response = app.clone().oneshot(register_request).await.unwrap();

    // check that registration was successful
    assert_eq!(register_response.status(), axum::http::StatusCode::CREATED);

    (username, password.to_string())
}
// helper function to log in a test user and return the access token and user_id
pub async fn login_test_user(app: &axum::Router, username: &str, password: &str) -> (uuid::Uuid, String) {
    let login_body = serde_json::json!({
        "identifier": username,
        "password": password,
    });

    // send a login request to get the JWT access token for this user
    let login_request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/users/login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(login_body.to_string()))
        .unwrap();

    let login_response = app.clone().oneshot(login_request).await.unwrap();

    // check that login was successful
    assert_eq!(login_response.status(), axum::http::StatusCode::OK);

    // parse the response body as JSON to get the access token
    let body = login_response.into_body().collect().await.unwrap();
    let body_bytes = body.to_bytes();

    #[derive(serde::Deserialize)]
    struct LoginResponse {
        user_id: uuid::Uuid,
        access_token: String,
    }

    let response: LoginResponse = serde_json::from_slice(&body_bytes).unwrap();

    (response.user_id, response.access_token)
}

// helper function to set up app state
pub async fn setup_app_state() -> AppState {
    // load .env variables from backend/.env (the working directory during tests is the workspace root)
    dotenvy::from_filename("backend/.env").ok();

    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
    let openai_api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let http_client = reqwest::Client::new();

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .unwrap();

    AppState {
        pool,
        jwt_secret: jwt_secret.clone(),
        openai_api_key: openai_api_key.clone(),
        http_client,
    }
}
