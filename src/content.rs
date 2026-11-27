use crate::Map;
use serde_json::{Value, json};

fn python_string(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(v) => if *v { "True" } else { "False" }.into(),
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        other => other.to_string(),
    }
}

fn format_tool_call(args: &Value) -> String {
    let parsed = match args {
        Value::String(text) => {
            serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        other => other.clone(),
    };
    let command = parsed.get("command").and_then(Value::as_str);
    let fallback = python_string(&parsed);
    let text = command.unwrap_or(&fallback).to_owned();
    format!("```\n{text}\n```")
}

fn format_observation(content: &str) -> String {
    let Ok(Value::Object(data)) = serde_json::from_str::<Value>(content) else {
        return content.to_owned();
    };
    if !data.contains_key("returncode") {
        return content.to_owned();
    }
    data.iter()
        .map(|(k, v)| format!("<{k}>\n{}", python_string(v)))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn get_content_string(message: &Value) -> String {
    let mut texts = Vec::new();
    if let Some(content) = message.get("content") {
        match content {
            Value::String(text) => texts.push(format_observation(text)),
            Value::Array(items) => {
                for item in items {
                    let Some(obj) = item.as_object() else {
                        continue;
                    };
                    match obj.get("type").and_then(Value::as_str) {
                        Some("tool_use") => {
                            texts.push(format_tool_call(obj.get("input").unwrap_or(&json!({}))))
                        }
                        Some("tool_result") => {
                            if let Some(Value::String(text)) = obj.get("content") {
                                texts.push(format_observation(text))
                            }
                        }
                        _ => {
                            if let Some(Value::String(text)) = obj.get("text") {
                                texts.push(text.clone())
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(Value::Array(calls)) = message.get("tool_calls") {
        for call in calls {
            if let Some(function) = call.get("function") {
                texts.push(format_tool_call(
                    function
                        .get("arguments")
                        .unwrap_or(&Value::String("{}".into())),
                ));
            }
        }
    }
    if let Some(output) = message.get("output") {
        match output {
            Value::String(text) => texts.push(format_observation(text)),
            Value::Array(items) => {
                for item in items {
                    let Some(obj) = item.as_object() else {
                        continue;
                    };
                    match obj.get("type").and_then(Value::as_str) {
                        Some("message") => {
                            if let Some(Value::Array(content)) = obj.get("content") {
                                for c in content {
                                    if let Some(Value::String(text)) = c.get("text") {
                                        texts.push(text.clone())
                                    }
                                }
                            }
                        }
                        Some("function_call") => texts.push(format_tool_call(
                            obj.get("arguments").unwrap_or(&Value::String("{}".into())),
                        )),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    texts
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn message_role(message: &Value) -> &str {
    message
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| message.get("type").and_then(Value::as_str))
        .unwrap_or("unknown")
}

pub fn message_extra(message: &Value) -> Map {
    message
        .get("extra")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}
pub fn message_extra_ref(message: &Value) -> Option<&Map> {
    message.get("extra").and_then(Value::as_object)
}

pub fn without_extra(message: &Value) -> Value {
    let mut copy = message.clone();
    if let Some(obj) = copy.as_object_mut() {
        obj.remove("extra");
    }
    copy
}
