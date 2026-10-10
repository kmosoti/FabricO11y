//! Console presentation models are native-testable; browser effects stay in app.
#![forbid(unsafe_code)]

pub mod live;
pub mod model;

#[cfg(target_arch = "wasm32")]
pub mod app;
