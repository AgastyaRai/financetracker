mod common;

use tower::util::ServiceExt;
use http_body_util::BodyExt;
use financetracker::{build_app, Transaction};

#[derive(serde::Deserialize)]
struct SemanticSearchResult {
    transactions: Vec<Transaction>,
    #[allow(dead_code)]
    summary: Option<String>,
}

// use the test module
#[cfg(test)]
mod semantic_search_tests {
    use super::*;

    // test to see if semantic search returns the most relevant transaction in an extremely obvious case
    #[tokio::test]
    async fn test_semantic_search_relevant_match() {
        // use our common helper functions to set up app state, load .env and register + log in a test user
        let state = common::setup_app_state().await;
        let app = build_app(state.clone());
        let (username, password) = common::create_and_register_test_user(&app).await;
        let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // add an Uber-related transaction
        let uber_transaction = serde_json::json!({
            "amount": 17.17,
            "kind": "Expense",
            "date": "2026-01-07",
            "category": "Transportation",
            "description": "Uber ride home from airport"
        });

        let uber_transaction_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/transactions")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(uber_transaction.to_string()))
            .unwrap();

        let uber_transaction_response = app.clone().oneshot(uber_transaction_request).await.unwrap();

        // print for debugging
        let status = uber_transaction_response.status();
        let body_bytes = uber_transaction_response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8_lossy(&body_bytes);

        println!("Add Uber transaction status: {}", status);
        println!("Add Uber transaction body: {}", body_str);

        assert_eq!(status, axum::http::StatusCode::CREATED);

        // now create a completely unrelated transaction
        let unrelated_transaction = serde_json::json!({
            "amount": 170.00,
            "kind": "Expense",
            "date": "2026-01-08",
            "category": "Entertainment",
            "description": "Concert tickets"
        });

        common::add_transaction(&app, &access_token, unrelated_transaction).await;

        // now perform semantic search
        let search_body = serde_json::json!({
            "query": "uber",
            "limit": 1
        });

        let search_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/transactions/search/semantic")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(search_body.to_string()))
            .unwrap();

        let search_response = app.clone().oneshot(search_request).await.unwrap();
        assert_eq!(search_response.status(), axum::http::StatusCode::OK);

        let body = search_response.into_body().collect().await.unwrap();
        let body_bytes = body.to_bytes();
        
        let result: SemanticSearchResult = serde_json::from_slice(&body_bytes).unwrap();
        let results = result.transactions;

        // the most relevant transaction (the Uber one) should be the first result returned
        let first_result_description = results[0].description.as_deref().unwrap_or("");

        assert_eq!(first_result_description, "Uber ride home from airport");
    }    

    // test to see if semantic search results are user-specific and don't return transactions from other users
    #[tokio::test]
    async fn test_semantic_search_user_specific() {
        // set up app state and register + log in first test user
        let state = common::setup_app_state().await;
        let app = build_app(state.clone());
        let (username1, password1) = common::create_and_register_test_user(&app).await;
        let (_user_id1, access_token1) = common::login_test_user(&app, &username1, &password1).await;

        // add a transaction for the first user
        let transaction_user1 = serde_json::json!({
            "amount": 117.17,
            "kind": "Expense",
            "date": "2026-01-17",
            "category": "Groceries",
            "description": "Groceries from NoFrills"
        });

        common::add_transaction(&app, &access_token1, transaction_user1).await;

        // set up and log in a second test user
        let (username2, password2) = common::create_and_register_test_user(&app).await;
        let (_user_id2, access_token2) = common::login_test_user(&app, &username2, &password2).await;

        // add a transaction for the second user that is similar to the first user's transaction
        let transaction_user2 = serde_json::json!({
            "amount": 217.17,
            "kind": "Expense",
            "date": "2026-01-17",
            "category": "Groceries",
            "description": "Groceries from SaveOn"
        });

        common::add_transaction(&app, &access_token2, transaction_user2).await;

        // now perform semantic search with the first user, using a query that should match both transactions
        let search_body_user1 = serde_json::json!({
            "query": "groceries",
            "limit": 7
        });

        let search_request_user1 = axum::http::Request::builder()
            .method("POST")
            .uri("/api/transactions/search/semantic")
            .header("Authorization", format!("Bearer {}", access_token1))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(search_body_user1.to_string()))
            .unwrap();

        let search_response_user1 = app.clone().oneshot(search_request_user1).await.unwrap();
        assert_eq!(search_response_user1.status(), axum::http::StatusCode::OK);

        let body_user1 = search_response_user1.into_body().collect().await.unwrap();
        let body_bytes_user1 = body_user1.to_bytes();
        
        let result: SemanticSearchResult = serde_json::from_slice(&body_bytes_user1).unwrap();
        let results = result.transactions;

        let descriptions_user1: Vec<&str> = results.iter().map(|t| t.description.as_deref().unwrap_or("")).collect();

        // only the first user's transaction should be returned in the search results, even though the second user's transaction is similar, because the search should be user-specific
        assert_eq!(descriptions_user1, vec!["Groceries from NoFrills"]);
    }

    // test to see if semantic search backfills missing embeddings for this user
    #[tokio::test]
    async fn test_semantic_search_backfills_missing_embeddings() {
        // set up app state and register + log in a test user
        let state = common::setup_app_state().await;
        let app = build_app(state.clone());
        let (username, password) = common::create_and_register_test_user(&app).await;
        let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // insert a transaction directly into the transactions table, bypassing the normal
        // add_transaction route so that no embedding row is created for it yet
        let inserted_transaction = sqlx::query!(
            "INSERT INTO transactions (user_id, amount, kind, category, date, description)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id",
            user_id,
            rust_decimal::Decimal::new(4242, 2),
            "expense",
            Some("Transportation".to_string()),
            chrono::NaiveDate::from_ymd_opt(2026, 1, 20).unwrap(),
            Some("Uber ride to campus".to_string())
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();

        let transaction_id = inserted_transaction.id;

        // confirm that there is no embedding row yet
        let existing_embedding = sqlx::query!(
            "SELECT transaction_id FROM transaction_embeddings WHERE transaction_id = $1",
            transaction_id
        )
        .fetch_optional(&state.pool)
        .await
        .unwrap();

        assert!(existing_embedding.is_none());

        // now perform semantic search, which should backfill the missing embedding first
        let search_body = serde_json::json!({
            "query": "uber",
            "limit": 5
        });

        let search_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/transactions/search/semantic")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(search_body.to_string()))
            .unwrap();

        let search_response = app.clone().oneshot(search_request).await.unwrap();
        assert_eq!(search_response.status(), axum::http::StatusCode::OK);

        let body = search_response.into_body().collect().await.unwrap();
        let body_bytes = body.to_bytes();

        let result: SemanticSearchResult = serde_json::from_slice(&body_bytes).unwrap();
        let results = result.transactions;

        // the transaction should now be searchable
        assert!(results.iter().any(|t| t.description.as_deref() == Some("Uber ride to campus")));

        // and the missing embedding row should have been created
        let stored_embedding = sqlx::query!(
            "SELECT embedding_text FROM transaction_embeddings WHERE transaction_id = $1",
            transaction_id
        )
        .fetch_optional(&state.pool)
        .await
        .unwrap();

        assert!(stored_embedding.is_some());

        let embedding_text = stored_embedding.unwrap().embedding_text;
        assert_eq!(
            embedding_text,
            "kind: Expense\n category: Transportation\n description: Uber ride to campus"
        );
    }

    // very general test to see if semantic search summary generation works (just checking that it returns
    // something since the output is non-deterministic)
    #[tokio::test]
    async fn test_semantic_search_summary_generation() {
        // use helper functions to set up app state, register + log in a test user
        let state = common::setup_app_state().await;
        let app = build_app(state.clone());
        let (username, password) = common::create_and_register_test_user(&app).await;
        let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // add  transaction for this user
        let transaction = serde_json::json!({
            "amount": 50.00,
            "kind": "Expense",
            "date": "2026-01-25",
            "category": "Food",
            "description": "Got dinner at Sushi Mugne"
        });

        common::add_transaction(&app, &access_token, transaction).await;

        // now perform semantic search with the summary option enabled
         let search_body = serde_json::json!({
            "query": "sushi",
            "limit": 5,
            "summary": true
        });

        let search_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/transactions/search/semantic")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(search_body.to_string()))
            .unwrap();

        let search_response = app.clone().oneshot(search_request).await.unwrap();
        assert_eq!(search_response.status(), axum::http::StatusCode::OK);

        let body = search_response.into_body().collect().await.unwrap();
        let body_bytes = body.to_bytes();

        let result: SemanticSearchResult = serde_json::from_slice(&body_bytes).unwrap();

        // since the summary is non-deterministic, we just check that it returns something
        assert!(result.summary.is_some());
    }
}