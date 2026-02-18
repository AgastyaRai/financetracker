// this lib.rs makes the binary's public items available for integration tests
// by exposing modules and re-exporting public items

pub mod app;
pub mod auth;
pub mod handlers;
pub mod models;

pub use app::build_app;
pub use auth::verify_jwt;
pub use models::AppState;

