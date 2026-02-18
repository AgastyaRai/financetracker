    use http_body_util::BodyExt;



// data structures for testing

// structs for deserializing JSON responses from the API
#[derive(Debug, serde::Deserialize)]
struct LoginResponse { 
    user_id: uuid::Uuid,
    access_token: String,
}

#[derive(Debug, serde::Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

/* tests */

// we create a separate module for testing, and only compile them when we run tests
#[cfg(test)]
mod jwt_tests {
    // import the app functions
    use super::*;
    use financetracker::{AppState, build_app};
    use tower::util::ServiceExt; // for oneshot

    // basic test to see if login works and returns a valid JWT
    #[tokio::test]
    async fn test_login_and_jwt() {

        // load .env variables for the test
        dotenvy::dotenv().ok();

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

        // set up app state using helper function
        let state = setup_app_state().await;

        // build the app router with the state
        let app = build_app(state);

        // create and register a test user and get the username and password with a helper function
        let (username, password) = create_and_register_test_user(&app).await;

        // now we try to log in with the same credentials
        let login_body = serde_json::json!({
            "identifier": username,
            "password": password,
        });

        let login_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/users/login")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(login_body.to_string()))
            .unwrap();

        let login_response = app.clone().oneshot(login_request).await.unwrap();

        // status code should be 200 OK
        assert_eq!(login_response.status(), axum::http::StatusCode::OK);

        // parse the response body as JSON to get the access token
        let body = login_response.into_body().collect().await.unwrap();
        let body_bytes = body.to_bytes();

        let login_response: LoginResponse = serde_json::from_slice(&body_bytes).unwrap();

        // validate the JWT using the same secret key and check the claims
        // manually set validation parameters to match what we set in the code
        
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        validation.validate_exp = true;

        let token_data = jsonwebtoken::decode::<Claims>(
            &login_response.access_token,
            &jsonwebtoken::DecodingKey::from_secret(jwt_secret.as_bytes()),
            &validation,
        ).unwrap();

        // the subject claim should be the user ID of the test user
        assert_eq!(token_data.claims.sub, login_response.user_id.to_string());

        // the expiration claim should be in the future
        let now_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;

        assert!(token_data.claims.exp > now_seconds);

    }

    // test to see if login with a non-existent user works and doesn't return a JWT
    #[tokio::test]
    async fn test_login_with_nonexistent_user() {
        // load .env variables for the test
        dotenvy::dotenv().ok();

        // set up app state using helper function
        let state = setup_app_state().await;
        
        // build the app router with the state
        let app = build_app(state);

        // try to log in with invalid credentials
        let login_body = serde_json::json!({
            "identifier": "nonexistentuser",
            "password": "wrongpassword",
        });

        let login_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/users/login")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(login_body.to_string()));

        // we expect this to return a 401 Unauthorized error, and not a JWT
        let login_response = app.clone().oneshot(login_request.unwrap()).await.unwrap();

        // status code should be 401 Unauthorized
        assert_eq!(login_response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    // test to see if login with incorrect password check works and doesn't return a JWT
    #[tokio::test]
    async fn test_login_with_incorrect_password() {
        // load .env variables for the test
        dotenvy::dotenv().ok();

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

        // set up app state using helper function
        let state = setup_app_state().await;

        // build the app router with the state
        let app = build_app(state);

        // register a test user and get the username and password with a helper function
        let (username, password) = create_and_register_test_user(&app).await;

        // now try to login to that account with an incorrect password
        let login_body = serde_json::json!({
            "identifier": username,
            "password": "wrongpassword",
        });

        let login_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/users/login")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(login_body.to_string()))
            .unwrap();

        // we expect this to return a 401 Unauthorized error, and not a JWT
        let login_response = app.clone().oneshot(login_request).await.unwrap();

        assert_eq!(login_response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    // check that trying to access a protected route without a JWT returns an error
    #[tokio::test]
    async fn test_access_protected_route_without_jwt() {
        // load .env variables for the test
        dotenvy::dotenv().ok();

        // set up app state using helper function
        let state = setup_app_state().await;

        // build the app router with the state
        let app = build_app(state);

        // try to access a protected route (e.g. get budgets for a user) without a JWT
        let user_id = uuid::Uuid::new_v4(); // we can use any random user ID here

        let request = axum::http::Request::builder()
            .method("GET")
            .uri(format!("/api/budgets/{}", user_id))
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // we expect this to return a 401 Unauthorized error since we're not providing a JWT
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);   
    }

    // check that trying to access a protected route with a different user's JWT returns an error
    #[tokio::test]
    async fn test_access_protected_route_with_wrong_jwt() {
        // load .env variables for the test
        dotenvy::dotenv().ok();

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

        // set up app state using helper function
        let state = setup_app_state().await;

        // build the app router with the state
        let app = build_app(state);

        // create and register a test user and get the username and password with a helper function
        let (username, password) = create_and_register_test_user(&app).await;

        // log in to that account to get a valid JWT
        let login_body = serde_json::json!({
            "identifier": username,
            "password": password,
        });

        let login_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/users/login")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(login_body.to_string()))
            .unwrap();

        let login_response = app.clone().oneshot(login_request).await.unwrap();

        // parse the response body as JSON to get the access token
        let body = login_response.into_body().collect().await.unwrap();
        let body_bytes = body.to_bytes();

        let login_response: LoginResponse = serde_json::from_slice(&body_bytes).unwrap();

        // now try to access a protected route (e.g. get budgets for a user) with the JWT from the test user, but for a different user ID
        let different_user_id = uuid::Uuid::new_v4(); // we can use any random user ID here that is different from the test user's ID

        let request = axum::http::Request::builder()
            .method("GET")
            .uri(format!("/api/budgets/{}", different_user_id))
            .header("Authorization", format!("Bearer {}", login_response.access_token))
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // we expect this to return a 401 Unauthorized error since the JWT is valid but for a different user
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    // check that accessing a protected route with a valid JWT for the correct user works successfully
    #[tokio::test]
    async fn test_access_protected_route_with_valid_jwt() {
        // load .env variables for the test
        dotenvy::dotenv().ok();

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

        // set up app state using helper function
        let state = setup_app_state().await;

        // build the app router with the state
        let app = build_app(state);

        // create and register a test user and get the username and password with a helper function
        let (username, password) = create_and_register_test_user(&app).await;

        // log in to that account to get a valid JWT
        let login_body = serde_json::json!({
            "identifier": username,
            "password": password,
        });

        let login_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/users/login")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(login_body.to_string()))
            .unwrap();

        let login_response = app.clone().oneshot(login_request).await.unwrap();

        // parse the response body as JSON to get the access token
        let body = login_response.into_body().collect().await.unwrap();
        let body_bytes = body.to_bytes();

        let login_response: LoginResponse = serde_json::from_slice(&body_bytes).unwrap();

        // now try to access a protected route (e.g. get budgets for a user) with the JWT from the test user, and the correct user ID
        let request = axum::http::Request::builder()
            .method("GET")
            .uri(format!("/api/budgets/{}", login_response.user_id))
            .header("Authorization", format!("Bearer {}", login_response.access_token))
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // we expect this to return a 200 OK status since the JWT is valid and for the correct user
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    // helper function to set up app state
    async fn setup_app_state() -> AppState {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .unwrap();

        AppState {
            pool,
            jwt_secret: jwt_secret.clone(),
        }
    }

    // helper function to create and register a unique test user and returns the username and password
    async fn create_and_register_test_user(app: &axum::Router) -> (String, String) {
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

    

}
