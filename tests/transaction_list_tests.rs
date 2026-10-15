mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use financetracker::build_app;
use http_body_util::BodyExt;
use tower::util::ServiceExt;
use uuid::Uuid;

#[derive(serde::Deserialize)]
struct ListedTransaction {
    id: Uuid,
}

#[tokio::test]
async fn test_list_excludes_removed_transactions_without_deleting_them() {
    // register and log in a separate user so only this test's records appear in the list
    let state = common::setup_app_state().await;
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, token) = common::login_test_user(&app, &username, &password).await;

    // store one imported transaction in each supported state and keep their IDs in the same order
    let mut transaction_ids = Vec::new();
    for status in ["pending", "posted", "removed"] {
        let transaction_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO transactions (user_id, amount, kind, date, origin, status)
             VALUES ($1, -10.00, 'expense', '2026-03-05', 'imported', $2)
             RETURNING id",
        )
        .bind(user_id)
        .bind(status)
        .fetch_one(&state.pool)
        .await
        .unwrap();

        transaction_ids.push(transaction_id);
    }
    let [pending_id, posted_id, removed_id]: [Uuid; 3] = transaction_ids.try_into().unwrap();

    // fetch the user's normal transaction list through the authenticated endpoint
    let request = Request::builder()
        .method("GET")
        .uri("/api/transactions")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let transactions: Vec<ListedTransaction> = serde_json::from_slice(&body).unwrap();

    // hiding the removed transaction must leave its historical row and status intact
    let removed_status =
        sqlx::query_scalar::<_, String>("SELECT status FROM transactions WHERE id = $1")
            .bind(removed_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(removed_status, "removed");

    // compare IDs without depending on database row order; pending and posted must both remain visible
    let mut returned_ids: Vec<Uuid> = transactions
        .iter()
        .map(|transaction| transaction.id)
        .collect();
    let mut expected_ids = vec![pending_id, posted_id];
    returned_ids.sort_unstable();
    expected_ids.sort_unstable();
    assert_eq!(
        returned_ids, expected_ids,
        "the list should contain pending and posted transactions, excluding the removed transaction"
    );
}
