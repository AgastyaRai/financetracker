mod common;

use tower::util::ServiceExt;
use http_body_util::BodyExt;
use financetracker::build_app;

#[derive(serde::Deserialize)]
struct BudgetProgress {
    category: String,
    budget_amount: rust_decimal::Decimal,
    spent: rust_decimal::Decimal,
    remaining: rust_decimal::Decimal,
}

// use the test module
#[cfg(test)]
mod budget_tests {
    use super::*;

    // see if the budget progress endpoint correctly calculates the spent amount using signed expense amounts
    #[tokio::test]
    async fn test_budget_progress_uses_signed_expense_amounts() {
        // use our common helper functions to set up app state, load .env and register + log in a test user
        let state = common::setup_app_state().await;
        let app = build_app(state);
        let (username, password) = common::create_and_register_test_user(&app).await;
        let (_user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // create a budget for the month being tested and post it to the API
        let budget = serde_json::json!({
            "month": "2026-03-01",
            "category": "Groceries",
            "amount": 100.00
        });

        let budget_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/budgets")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(budget.to_string()))
            .unwrap();

        let budget_response = app.clone().oneshot(budget_request).await.unwrap();
        assert_eq!(budget_response.status(), axum::http::StatusCode::CREATED);

        // add an expense outflow, an expense refund, and a non-expense outflow
        let expense = serde_json::json!({
            "amount": -80.00,
            "kind": "Expense",
            "date": "2026-03-05",
            "category": "Groceries",
            "description": "Groceries"
        });

        common::add_transaction(&app, &access_token, expense).await;

        let refund = serde_json::json!({
            "amount": 20.00,
            "kind": "Expense",
            "date": "2026-03-06",
            "category": "Groceries",
            "description": "Grocery refund"
        });

        common::add_transaction(&app, &access_token, refund).await;

        let income = serde_json::json!({
            "amount": -30.00,
            "kind": "Income",
            "date": "2026-03-07",
            "category": "Groceries",
            "description": "Non-expense outflow"
        });

        common::add_transaction(&app, &access_token, income).await;

        // get the budget progress for the month
        let progress_request = axum::http::Request::builder()
            .method("GET")
            .uri("/api/budgets/progress?month=2026-03-01")
            .header("Authorization", format!("Bearer {}", access_token))
            .body(axum::body::Body::empty())
            .unwrap();

        let progress_response = app.clone().oneshot(progress_request).await.unwrap();
        assert_eq!(progress_response.status(), axum::http::StatusCode::OK);

        let body = progress_response.into_body().collect().await.unwrap();
        let body_bytes = body.to_bytes();
        let progress: Vec<BudgetProgress> = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].category, "Groceries");
        assert_eq!(progress[0].budget_amount, rust_decimal::Decimal::new(10000, 2));
        assert_eq!(progress[0].spent, rust_decimal::Decimal::new(6000, 2));
        assert_eq!(progress[0].remaining, rust_decimal::Decimal::new(4000, 2));
    }

    // see if budget progress report nothing spent when expense refunds are greater than expense outflows
    #[tokio::test]
    async fn test_budget_progress_does_not_return_negative_spending() {
        // use our common helper functions to set up app state, load .env and register + log in a test user
        let state = common::setup_app_state().await;
        let app = build_app(state);
        let (username, password) = common::create_and_register_test_user(&app).await;
        let (_user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // create a budget for the month being tested and post it to the API
        let budget = serde_json::json!({
            "month": "2026-03-01",
            "category": "Groceries",
            "amount": 50.00
        });

        let budget_request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/budgets")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(budget.to_string()))
            .unwrap();

        let budget_response = app.clone().oneshot(budget_request).await.unwrap();
        assert_eq!(budget_response.status(), axum::http::StatusCode::CREATED);

        // add an expense outflow and a larger expense refund
        let expense = serde_json::json!({
            "amount": -10.00,
            "kind": "Expense",
            "date": "2026-03-05",
            "category": "Groceries",
            "description": "Groceries"
        });

        common::add_transaction(&app, &access_token, expense).await;

        let refund = serde_json::json!({
            "amount": 25.00,
            "kind": "Expense",
            "date": "2026-03-06",
            "category": "Groceries",
            "description": "Grocery refund"
        });

        common::add_transaction(&app, &access_token, refund).await;

        // get the budget progress for the month
        let progress_request = axum::http::Request::builder()
            .method("GET")
            .uri("/api/budgets/progress?month=2026-03-01")
            .header("Authorization", format!("Bearer {}", access_token))
            .body(axum::body::Body::empty())
            .unwrap();

        let progress_response = app.clone().oneshot(progress_request).await.unwrap();
        assert_eq!(progress_response.status(), axum::http::StatusCode::OK);

        let body = progress_response.into_body().collect().await.unwrap();
        let body_bytes = body.to_bytes();
        let progress: Vec<BudgetProgress> = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].category, "Groceries");
        assert_eq!(progress[0].budget_amount, rust_decimal::Decimal::new(5000, 2));
        assert_eq!(progress[0].spent, rust_decimal::Decimal::ZERO);
        assert_eq!(progress[0].remaining, rust_decimal::Decimal::new(5000, 2));
    }
}
