use crate::{Map, Result, error::value_error};
use include_dir::{Dir, include_dir};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

static CONFIG_DIR: Dir = include_dir!("config");

pub fn builtin_config_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config")
}

pub fn builtin_config(name: &str) -> Result<String> {
    CONFIG_DIR
        .get_file(name)
        .and_then(|f| f.contents_utf8())
        .map(str::to_owned)
        .ok_or_else(|| value_error(format!("Could not find built-in config {name}")))
}

pub fn get_config_path(spec: &str) -> Result<PathBuf> {
    let mut path = PathBuf::from(spec);
    if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
        path.set_extension("yaml");
    }
    let mut candidates = vec![path.clone()];
    if let Ok(dir) = env::var("MSWEA_CONFIG_DIR") {
        candidates.push(Path::new(&dir).join(&path));
    }
    candidates.push(builtin_config_dir().join(&path));
    candidates.push(builtin_config_dir().join("extra").join(&path));
    candidates.push(builtin_config_dir().join("benchmarks").join(&path));
    let display = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        value_error(format!(
            "Could not find config file for {spec} (tried: {display})"
        ))
    })
}

pub fn key_value_spec_to_nested_dict(spec: &str) -> Result<Map> {
    let (key, raw) = spec
        .split_once('=')
        .ok_or_else(|| value_error(format!("Invalid config spec {spec:?}: missing '='")))?;
    let keys: Vec<_> = key.split('.').collect();
    if keys.iter().any(|k| k.is_empty()) {
        return Err(value_error(format!(
            "Invalid config spec {spec:?}: empty config key"
        )));
    }
    let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
    let mut root = Map::new();
    let mut current = &mut root;
    for key in &keys[..keys.len() - 1] {
        current.insert((*key).to_owned(), Value::Object(Map::new()));
        current = current.get_mut(*key).unwrap().as_object_mut().unwrap();
    }
    current.insert(keys[keys.len() - 1].to_owned(), value);
    Ok(root)
}

pub fn get_config_from_spec(spec: &str) -> Result<Value> {
    if spec.contains('=') {
        return Ok(Value::Object(key_value_spec_to_nested_dict(spec)?));
    }
    if let Ok(path) = get_config_path(spec) {
        let text = fs::read_to_string(&path)
            .map_err(|e| value_error(format!("{}: {e}", path.display())))?;
        return Ok(serde_yaml::from_str(&text)?);
    }
    let mut name = PathBuf::from(spec);
    if name.extension().and_then(|s| s.to_str()) != Some("yaml") {
        name.set_extension("yaml");
    }
    for prefix in ["", "extra/", "benchmarks/"] {
        let embedded = format!("{prefix}{}", name.to_string_lossy().replace('\\', "/"));
        if let Some(text) = CONFIG_DIR
            .get_file(&embedded)
            .and_then(|f| f.contents_utf8())
        {
            return Ok(serde_yaml::from_str(text)?);
        }
    }
    Err(get_config_path(spec).unwrap_err())
}

pub fn recursive_merge(values: impl IntoIterator<Item = Value>) -> Value {
    let mut result = Map::new();
    for value in values {
        let Value::Object(object) = value else {
            continue;
        };
        for (key, value) in object {
            match value {
                Value::Object(new) => {
                    let merged = match result.remove(&key) {
                        Some(Value::Object(old)) => {
                            recursive_merge([Value::Object(old), Value::Object(new)])
                        }
                        _ => recursive_merge([Value::Object(new)]),
                    };
                    result.insert(key, merged);
                }
                value => {
                    result.insert(key, value);
                }
            }
        }
    }
    Value::Object(result)
}

pub fn with_defaults(config: Map, defaults: Value) -> Map {
    let Value::Object(mut merged) = defaults else {
        return config;
    };
    for (key, value) in config {
        merged.insert(key, value);
    }
    merged
}

pub fn get_map(config: &Value, key: &str) -> Map {
    config
        .get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}
pub fn nested<'a>(config: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(config, |v, k| v.get(k))
}

pub fn insert_cli_overrides(config: &mut Map, path: &str, value: Option<Value>) {
    let Some(value) = value else { return };
    let mut current = config;
    let keys: Vec<_> = path.split('.').collect();
    for key in &keys[..keys.len() - 1] {
        let entry = current
            .entry((*key).to_owned())
            .or_insert_with(|| json!({}));
        if !entry.is_object() {
            *entry = json!({});
        }
        current = entry.as_object_mut().unwrap();
    }
    current.insert(keys[keys.len() - 1].to_owned(), value);
}
