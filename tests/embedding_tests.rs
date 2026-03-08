mod common;
use tower::util::ServiceExt; // for oneshot
use sqlx::Row; // for row.get()
use pgvector::Vector;
use http_body_util::BodyExt; // for .collect()

// this goes into the test module
#[cfg(test)]
mod embedding_tests {
    // import the app functions and models
    use super::*;
    use financetracker::build_app;

    // test the embedding generation function with a sample transaction
    #[tokio::test]
    async fn test_embedding_generation() {
        // use common helper function to set up app state (also loads .env)
        let state = common::setup_app_state().await;

        // use helper function to create and register a test user, and get the username and password
        let (username, password) = common::create_and_register_test_user(&build_app(state.clone())).await;

        // now, we try to add a transaction for this user, which will trigger the embedding generation in the handler
        let (user_id, access_token) = common::login_test_user(&build_app(state.clone()), &username, &password).await;

        let add_transaction_body = serde_json::json!({
            "amount": 12.34,
            "kind": "Expense",
            "date": "2026-01-01",
            "category": "Food",
            "description": "Dinner at restaurant"
        });

        let add_transaction_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/transactions")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(add_transaction_body.to_string()))
            .unwrap();

        let add_transaction_response = build_app(state.clone()).oneshot(add_transaction_request).await.unwrap();

        let status = add_transaction_response.status();
        let body_bytes = add_transaction_response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8_lossy(&body_bytes);
        println!("Add transaction status: {}", status);
        println!("Add transaction body: {}", body_str);

        // check that the transaction was added successfully
        assert_eq!(status, axum::http::StatusCode::CREATED);

        // now, we query the database directly to check that the embedding was generated and stored correctly
        let expected_embedding_text = "kind: Expense\n category: Food\n description: Dinner at restaurant";

        let row = sqlx::query(
            "SELECT embedding_text, embedding FROM transaction_embeddings WHERE user_id = $1 AND embedding_text = $2"
        )
        .bind(user_id)
        .bind(expected_embedding_text)
        .fetch_one(&state.pool)
        .await
        .expect("Failed to fetch embedding from database");

        // check that the embedding and embe text is correctdding
        let embedding: Vector = row.get("embedding");
        let embedding_text: String = row.get("embedding_text");

        assert_eq!(embedding.to_vec().len(), 1536); // should be 1536 dimensions for text-embedding-3-small
        assert_eq!(embedding_text, expected_embedding_text);

        

    }
}