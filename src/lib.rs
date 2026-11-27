pub mod agents;
pub mod config;
pub mod content;
pub mod environments;
pub mod error;
pub mod models;
pub mod run;
pub mod template;
pub mod utils;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub type Json = serde_json::Value;
pub type Map = serde_json::Map<String, Json>;
pub type Result<T> = std::result::Result<T, error::MiniError>;
