// this lib.rs makes the binary's public items available for integration tests
// by simply re-exporting the main module, which contains the app state and build_app function
// this way, we can import these items in our tests without having to run the whole server or duplicate code

#[path = "main.rs"]
mod main_module;

pub use main_module::{AppState, build_app, verify_jwt};
