// this lib.rs makes the binary's public items available for integration tests
// by exposing modules and re-exporting public items

pub mod ai;
pub mod app;
pub mod bank_credentials;
pub mod auth;
pub mod embeddings;
pub mod embedding_worker;
pub mod handlers;
mod imports;
pub mod models;
pub mod plaid;

pub use app::build_app;
pub use auth::verify_jwt;
pub use models::AppState;
pub use embeddings::{generate_transaction_embedding, store_transaction_embedding};
pub use models::Transaction;
pub use ai::generate_semantic_search_summary;
