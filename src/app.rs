use crate::models::AppState;
use crate::handlers::*;

// router function to set up all the routes
pub fn build_app(state: AppState) -> axum::Router {
    
    use tower_http::cors::{CorsLayer, Any};
    use tower_http::services::{ServeDir, ServeFile};

    // add a cors layer to allow requests from any origin (for development purposes)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);


    // now, we set up our router

    // set up the api routes separately
    let api = axum::Router::new()
        // testing routes
        .route("/test", axum::routing::get(test_handler))
        .route("/test_state", axum::routing::get(test_state_handler))
        .route("/test_db", axum::routing::get(test_db_handler))

        // user routes
        .route("/users/register", axum::routing::post(register_user))
        .route("/users/login", axum::routing::post(user_login))

        // bank connection routes
        .route("/bank-connections", axum::routing::get(get_bank_connections))
        .route("/bank-connections/link-token", axum::routing::post(create_bank_link_token))
        .route("/bank-connections/exchange-token", axum::routing::post(exchange_bank_token))
        .route("/bank-connections/:id/import", axum::routing::post(import_bank_transactions))

        // transaction routes
        .route("/transactions", axum::routing::post(add_transaction))
        .route("/transactions", axum::routing::get(get_transactions))
        .route("/transactions/indexing-status", axum::routing::get(get_indexing_status))
        .route("/transactions/:id", axum::routing::put(update_transaction))
        .route("/transactions/:id/revert", axum::routing::post(revert_transaction))

        // budget routes
        .route("/budgets", axum::routing::post(upsert_budget))
        .route("/budgets", axum::routing::get(get_budgets))
        .route("/budgets/progress", axum::routing::get(get_budget_progress))

        // semantic search routes
        .route("/transactions/search/semantic", axum::routing::post(semantic_transaction_search))


        // layer with CORS for development
        .layer(cors)
        .with_state(state);

    // we nest the api under /api 
    axum::Router::new()
        .nest("/api", api)
        // serve the frontend static files from ./frontend/dist
        .fallback_service(
            ServeDir::new("./frontend/dist")
                .fallback(ServeFile::new("./frontend/dist/index.html")),
        )
}
