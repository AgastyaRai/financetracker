    mod common;
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
    use super::common;
    use financetracker::{AppState, build_app};
    use tower::util::ServiceExt; // for oneshot

    // basic test to see if login works and returns a valid JWT
    #[tokio::test]
    async fn test_login_and_jwt() {

        // set up app state using helper function (also loads .env)
        let state = common::setup_app_state().await;

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

        // build the app router with the state
        let app = build_app(state);

        // create and register a test user and get the username and password with a helper function
        let (username, password) = common::create_and_register_test_user(&app).await;

        // log in using helper and get the user_id and access token
        let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // validate the JWT using the same secret key and check the claims
        // manually set validation parameters to match what we set in the code
        
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        validation.validate_exp = true;

        let token_data = jsonwebtoken::decode::<Claims>(
            &access_token,
            &jsonwebtoken::DecodingKey::from_secret(jwt_secret.as_bytes()),
            &validation,
        ).unwrap();

        // the subject claim should be the user ID of the test user
        assert_eq!(token_data.claims.sub, user_id.to_string());

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
        // set up app state using helper function (also loads .env)
        let state = common::setup_app_state().await;
        
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
        // set up app state using helper function (also loads .env)
        let state = common::setup_app_state().await;

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

        // build the app router with the state
        let app = build_app(state);

        // register a test user and get the username and password with a helper function
        let (username, password) = common::create_and_register_test_user(&app).await;

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
        // set up app state using helper function (also loads .env)
        let state = common::setup_app_state().await;

        // build the app router with the state
        let app = build_app(state);

        // try to access a protected route (e.g. get budgets) without a JWT
        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/api/budgets")
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // we expect this to return a 401 Unauthorized error since we're not providing a JWT
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);   
    }

    // check that accessing a protected route with a valid JWT works successfully
    #[tokio::test]
    async fn test_access_protected_route_with_valid_jwt() {
        // set up app state using helper function (also loads .env)
        let state = common::setup_app_state().await;

        // build the app router with the state
        let app = build_app(state);

        // create and register a test user and get the username and password with a helper function
        let (username, password) = common::create_and_register_test_user(&app).await;

        // log in to that account to get a valid JWT
        let (_user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // now try to access a protected route (e.g. get budgets) with a valid JWT
        // the user_id is automatically extracted from the JWT token on the server
        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/api/budgets")
            .header("Authorization", format!("Bearer {}", access_token))
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // we expect this to return a 200 OK status since the JWT is valid
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

}

