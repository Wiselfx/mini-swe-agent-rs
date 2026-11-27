pub mod benchmarks;
pub mod init;
pub use init::{init, report_error};
pub mod logging;
pub mod utilities;

use crate::{Json, Map};
use serde_json::Value;
use std::{env, path::PathBuf};

pub fn env_flag(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => !matches!(
            value.trim().to_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        Err(_) => default,
    }
}

pub fn global_config_dir() -> PathBuf {
    env::var("MSWEA_GLOBAL_CONFIG_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| dirs::config_dir().map(|p| p.join("mini-swe-agent")))
        .unwrap_or_else(|| PathBuf::from(".mini-swe-agent"))
}

pub fn global_config_file() -> PathBuf {
    global_config_dir().join(".env")
}

pub fn load_global_config() {
    let path = global_config_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = dotenvy::from_path(path);
}

pub fn nested_map<'a>(config: &'a Json, key: &str) -> Option<&'a Map> {
    config.get(key).and_then(Value::as_object)
}
