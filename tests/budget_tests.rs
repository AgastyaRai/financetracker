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
    use rust_decimal::Decimal;

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

    #[tokio::test]
    async fn test_budget_progress_excludes_removed_transactions() {
        // register and log in a separate user for this budget and its imported expenses
        let state = common::setup_app_state().await;
        let app = build_app(state.clone());
        let (username, password) = common::create_and_register_test_user(&app).await;
        let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // create a $100 budget through the API for the category and month being tested
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

        // pending and posted expenses contribute $50; the removed $30 expense must not contribute
        for (status, amount) in [
            ("posted", Decimal::new(-4000, 2)),
            ("pending", Decimal::new(-1000, 2)),
            ("removed", Decimal::new(-3000, 2)),
        ] {
            sqlx::query(
                "INSERT INTO transactions (user_id, amount, kind, category, date, origin, status)
                 VALUES ($1, $2, 'expense', 'Groceries', '2026-03-05', 'imported', $3)",
            )
            .bind(user_id)
            .bind(amount)
            .bind(status)
            .execute(&state.pool)
            .await
            .unwrap();
        }

        // read the monthly calculation through the authenticated budget-progress endpoint
        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/api/budgets/progress?month=2026-03-01")
            .header("Authorization", format!("Bearer {}", access_token))
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let progress: Vec<BudgetProgress> = serde_json::from_slice(&body).unwrap();

        // excluding an expense from the calculation must preserve the removed historical row
        let removed_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM transactions WHERE user_id = $1 AND status = 'removed'",
        )
        .bind(user_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(removed_count, 1);

        // only the posted and pending expenses should reduce the available budget
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].category, "Groceries");
        assert_eq!(progress[0].budget_amount, Decimal::new(10000, 2));
        assert_eq!(progress[0].spent, Decimal::new(5000, 2));
        assert_eq!(progress[0].remaining, Decimal::new(5000, 2));
    }

    #[tokio::test]
    async fn test_budget_progress_keeps_category_with_only_removed_transactions() {
        // create a separate user so this category has exactly one expense, already marked removed
        let state = common::setup_app_state().await;
        let app = build_app(state.clone());
        let (username, password) = common::create_and_register_test_user(&app).await;
        let (user_id, access_token) = common::login_test_user(&app, &username, &password).await;

        // create a $50 budget even though it will have no eligible expenses after filtering
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

        // store the removed expense directly so the join must exclude it without dropping its budget
        sqlx::query(
            "INSERT INTO transactions (user_id, amount, kind, category, date, origin, status)
             VALUES ($1, -20.00, 'expense', 'Groceries', '2026-03-05', 'imported', 'removed')",
        )
        .bind(user_id)
        .execute(&state.pool)
        .await
        .unwrap();

        // request the category's budget progress through the normal authenticated route
        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/api/budgets/progress?month=2026-03-01")
            .header("Authorization", format!("Bearer {}", access_token))
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let progress: Vec<BudgetProgress> = serde_json::from_slice(&body).unwrap();

        // retain the removed transaction in the database while omitting its amount from spending
        let removed_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM transactions WHERE user_id = $1 AND status = 'removed'",
        )
        .bind(user_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(removed_count, 1);

        // the left join must still return this budget with nothing spent and the full amount remaining
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].category, "Groceries");
        assert_eq!(progress[0].budget_amount, Decimal::new(5000, 2));
        assert_eq!(progress[0].spent, Decimal::ZERO);
        assert_eq!(progress[0].remaining, Decimal::new(5000, 2));
    }
}
