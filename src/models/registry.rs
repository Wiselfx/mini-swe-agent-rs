use crate::{Json, Map, config::builtin_config};
use serde_json::Value;
use std::sync::OnceLock;

static REGISTRY: OnceLock<Map> = OnceLock::new();

pub fn model_info(name: &str) -> Option<&'static Map> {
    registry().get(name).and_then(Value::as_object)
}

fn registry() -> &'static Map {
    REGISTRY.get_or_init(|| {
        builtin_config("model_prices_and_context_window.json")
            .ok()
            .and_then(|text| serde_json::from_str::<Json>(&text).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    })
}

pub fn usage(response: &Value) -> Map {
    let Some(usage) = response.get("usage").filter(|v| v.is_object()) else {
        return Map::new();
    };
    let get = |names: &[&str]| -> i64 {
        for name in names {
            if let Some(value) = usage.get(*name).and_then(Value::as_i64) {
                return value;
            }
            if let Some(value) = usage.get(*name).and_then(Value::as_f64) {
                return value as i64;
            }
        }
        0
    };
    let details = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"));
    let cached = get(&["cache_read_input_tokens"]).max(
        details
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
    );
    Map::from_iter([
        (
            "prompt_tokens".into(),
            Value::from(get(&["prompt_tokens", "input_tokens"])),
        ),
        (
            "completion_tokens".into(),
            Value::from(get(&["completion_tokens", "output_tokens"])),
        ),
        ("cached_tokens".into(), Value::from(cached)),
        (
            "cache_write_tokens".into(),
            Value::from(get(&["cache_creation_input_tokens"])),
        ),
    ])
}

pub fn model_info_with_custom<'a>(name: &'a str, custom: Option<&'a Json>) -> Option<&'a Map> {
    custom
        .and_then(Json::as_object)
        .and_then(|m| m.get(name))
        .and_then(Value::as_object)
        .or_else(|| model_info(name))
}
