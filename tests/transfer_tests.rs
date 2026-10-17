mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use financetracker::build_app;
use http_body_util::BodyExt;
use rust_decimal::Decimal;
use tower::util::ServiceExt;
use uuid::Uuid;

#[derive(serde::Deserialize)]
struct ReturnedTransaction {
    id: Uuid,
    amount: Decimal,
    kind: String,
}

#[tokio::test]
async fn test_list_transfers_preserves_kind_and_signed_amounts() {
    // register and log in a separate user so the list contains only this test's transfers
    let state = common::setup_app_state().await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, token) = common::login_test_user(&app, &username, &password).await;

    // seed stored transfers directly, representing both directions of an imported account movement
    // keep the returned IDs and original amounts so the API response can be checked without relying on row order
    let mut expected_transfers = Vec::new();
    for amount in [Decimal::new(-10000, 2), Decimal::new(10000, 2)] {
        let transaction_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO transactions (user_id, amount, kind, date, origin, description)
             VALUES ($1, $2, 'transfer', '2026-03-05', 'imported', 'Transfer between own accounts')
             RETURNING id",
        )
        .bind(user_id)
        .bind(amount)
        .fetch_one(&state.pool)
        .await
        .unwrap();

        expected_transfers.push((transaction_id, amount));
    }

    // request the stored rows through the authenticated endpoint, including its database-to-enum conversion
    let request = Request::builder()
        .method("GET")
        .uri("/api/transactions")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let transactions: Vec<ReturnedTransaction> = serde_json::from_slice(&body).unwrap();

    // both transfers must retain their classification and signed amounts when serialized by the API
    assert_eq!(transactions.len(), expected_transfers.len());
    for (transaction_id, amount) in expected_transfers {
        let transaction = transactions
            .iter()
            .find(|transaction| transaction.id == transaction_id)
            .expect("the response should contain each stored transfer");

        assert_eq!(transaction.kind, "Transfer");
        assert_eq!(transaction.amount, amount);
    }
}
