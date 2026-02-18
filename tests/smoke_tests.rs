use http_body_util::BodyExt;

/* tests */

// we create a separate module for testing, and only compile them when we run tests
#[cfg(test)]
mod tests {
    // import the app functions
    use super::*;
    use financetracker::{AppState, build_app};
    use tower::util::ServiceExt; // for oneshot
    

    // smoke test
    #[tokio::test]
    async fn test_smoke() {
        // just test that cargo test works and we can run a test
        assert!(true);
    }

    // basic api test, see if test route works
    #[tokio::test]
    async fn test_api_test_route() {
        // set up minimal app state
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        
        // set up a dummy pool (we won't actually use it in this test, but the app needs it)
        // we also connect lazily here so we don't have to rely on the database being up,
        // since we're using a dummy pool anyways
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&db_url)
            .unwrap();  

        let state = AppState {
            pool,
            jwt_secret: "test_secret".to_string(),
        };

        // build the app router with the state
        let app = build_app(state);

        // make a request to the /api/test route
        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/api/test")
            .body(axum::body::Body::empty())
            .unwrap();

        // send the request through the router with a one-shot service so we don't have to run the whole server
        let response = app.oneshot(request).await.unwrap();

        // the status code should be 200 OK
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        // the body should be "Test route is working!"
        let body = response.into_body().collect().await.unwrap();
        let body_bytes = body.to_bytes();
        let body_str = std::str::from_utf8(&body_bytes).unwrap();

        assert_eq!(body_str, "Test route is working!");

    }

    // jwt testing

}