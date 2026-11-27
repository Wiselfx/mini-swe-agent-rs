use crate::{
    Json, Map, Result,
    content::without_extra,
    environments::CommandOutput,
    error::{ErrorKind, MiniError},
    template::render,
};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn bash_tool() -> Json {
    json!({"type":"function","function":{"name":"bash","description":"Execute a bash command","parameters":{"type":"object","properties":{"command":{"type":"string","description":"The bash command to execute"}},"required":["command"]}}})
}

pub fn bash_tool_response() -> Json {
    json!({"type":"function","name":"bash","description":"Execute a bash command","parameters":{"type":"object","properties":{"command":{"type":"string","description":"The bash command to execute"}},"required":["command"]}})
}

pub fn timestamp() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn render_error(
    template: &str,
    error: String,
    actions: Vec<Json>,
    has_tool_calls: bool,
    finish_reason: Option<&Value>,
) -> Result<String> {
    let mut vars = Map::new();
    vars.insert("error".into(), json!(error));
    vars.insert("actions".into(), json!(actions));
    vars.insert("has_tool_calls".into(), json!(has_tool_calls));
    if let Some(reason) = finish_reason {
        vars.insert("finish_reason".into(), reason.clone());
    }
    render(template, &vars)
}

pub fn parse_toolcall_actions(
    tool_calls: &[Value],
    format_error_template: &str,
    finish_reason: Option<&Value>,
) -> Result<Vec<Json>> {
    if tool_calls.is_empty() {
        let error = "No tool calls found in the response. Every response MUST include at least one tool call.";
        let content = render_error(
            format_error_template,
            error.into(),
            vec![],
            false,
            finish_reason,
        )?;
        return Err(MiniError::new(
            ErrorKind::Format,
            json!({"role":"user","content":content,"extra":{"interrupt_type":"FormatError"}}),
        ));
    }
    let mut actions = Vec::new();
    for call in tool_calls {
        let function = call.get("function").unwrap_or(&Value::Null);
        let name = function.get("name").and_then(Value::as_str).unwrap_or("");
        let empty_args = Value::String("{}".into());
        let raw_args = function.get("arguments").unwrap_or(&empty_args);
        let mut error = String::new();
        let args = match raw_args {
            Value::String(text) => match serde_json::from_str::<Value>(text) {
                Ok(value) => value,
                Err(e) => {
                    error.push_str(&format!("Error parsing tool call arguments: {e}."));
                    json!({})
                }
            },
            other => other.clone(),
        };
        if name != "bash" {
            error.push_str(&format!("Unknown tool '{name}'."));
        }
        if !args.is_object() || args.get("command").is_none() {
            error.push_str("Missing 'command' argument in bash tool call.");
        }
        if !error.is_empty() {
            let content = render_error(
                format_error_template,
                error.trim().into(),
                vec![],
                true,
                finish_reason,
            )?;
            return Err(MiniError::new(
                ErrorKind::Format,
                json!({"role":"user","content":content,"extra":{"interrupt_type":"FormatError","has_tool_calls":true}}),
            ));
        }
        actions.push(json!({"command": args["command"], "tool_call_id": call.get("id")}));
    }
    Ok(actions)
}

pub fn parse_regex_actions(
    content: &str,
    action_regex: &str,
    format_error_template: &str,
    finish_reason: Option<&Value>,
) -> Result<Vec<Json>> {
    let re = regex::Regex::new(action_regex)?;
    let actions: Vec<Json> = re
        .captures_iter(content)
        .filter_map(|c| c.get(1).map(|m| json!({"command":m.as_str().trim()})))
        .collect();
    if actions.len() != 1 {
        let error = format!("Expected exactly 1 action, found {}.", actions.len());
        let rendered = render_error(
            format_error_template,
            error,
            actions.clone(),
            false,
            finish_reason,
        )?;
        return Err(MiniError::new(
            ErrorKind::Format,
            json!({
                "role":"user","content":rendered,
                "extra":{"interrupt_type":"FormatError","n_actions":actions.len(),"model_response":content}
            }),
        ));
    }
    Ok(actions)
}

fn observation_vars(output: &CommandOutput, template_vars: &Map) -> Map {
    let mut vars = template_vars.clone();
    vars.insert("output".into(), output.to_json());
    vars
}

fn observation_extra(output: &CommandOutput) -> Map {
    let mut extra = Map::new();
    extra.insert("raw_output".into(), json!(output.output));
    extra.insert("returncode".into(), json!(output.returncode));
    extra.insert("timestamp".into(), json!(timestamp()));
    extra.insert("exception_info".into(), json!(output.exception_info));
    if let Some(output_extra) = &output.extra {
        extra.extend(output_extra.clone());
    }
    extra
}

pub fn format_observation_messages(
    outputs: &[CommandOutput],
    observation_template: &str,
    template_vars: &Map,
    multimodal_regex: &str,
) -> Result<Vec<Json>> {
    outputs.iter().map(|output| {
        let content = render(observation_template, &observation_vars(output, template_vars))?;
        Ok(json!({"role":"user","content":expand_multimodal(json!(&content), multimodal_regex)?,"extra":observation_extra(output)}))
    }).collect()
}

pub fn format_toolcall_observation_messages(
    actions: &[Json],
    outputs: &[CommandOutput],
    observation_template: &str,
    template_vars: &Map,
    multimodal_regex: &str,
) -> Result<Vec<Json>> {
    format_toolcall_observation_messages_impl(
        actions,
        outputs,
        observation_template,
        template_vars,
        multimodal_regex,
        false,
    )
}

pub fn format_toolcall_observation_messages_impl(
    actions: &[Json],
    outputs: &[CommandOutput],
    observation_template: &str,
    template_vars: &Map,
    multimodal_regex: &str,
    response_api: bool,
) -> Result<Vec<Json>> {
    let not_executed = CommandOutput {
        output: String::new(),
        returncode: -1,
        exception_info: "action was not executed".into(),
        extra: None,
    };
    let mut messages = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        let output = outputs.get(index).unwrap_or(&not_executed);
        let content = render(
            observation_template,
            &observation_vars(output, template_vars),
        )?;
        let mut message = Map::new();
        message.insert("extra".into(), json!(observation_extra(output)));
        if let Some(id) = action.get("tool_call_id").filter(|v| !v.is_null()) {
            if response_api {
                message.insert("type".into(), json!("function_call_output"));
                message.insert("call_id".into(), id.clone());
                message.insert("output".into(), json!(content));
            } else {
                message.insert("tool_call_id".into(), id.clone());
                message.insert("role".into(), json!("tool"));
                message.insert(
                    "content".into(),
                    expand_multimodal(json!(content), multimodal_regex)?,
                );
            }
        } else if response_api {
            message.insert("type".into(), json!("message"));
            message.insert("role".into(), json!("user"));
            message.insert(
                "content".into(),
                json!([{"type":"input_text","text":content}]),
            );
        } else {
            message.insert("role".into(), json!("user"));
            message.insert(
                "content".into(),
                expand_multimodal(json!(content), multimodal_regex)?,
            );
        }
        messages.push(Value::Object(message));
    }
    Ok(messages)
}

pub fn parse_response_actions(
    output: &[Value],
    format_error_template: &str,
    finish_reason: Option<&Value>,
) -> Result<Vec<Json>> {
    let calls: Vec<_> = output
        .iter()
        .filter(|v| v.get("type").and_then(Value::as_str) == Some("function_call"))
        .collect();
    if calls.is_empty() {
        let error = "No tool calls found in the response. Every response MUST include at least one tool call.";
        let text = render_error(
            format_error_template,
            error.into(),
            vec![],
            false,
            finish_reason,
        )?;
        return Err(MiniError::new(
            ErrorKind::Format,
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":text}],"extra":{"interrupt_type":"FormatError"}}),
        ));
    }
    let mut actions = Vec::new();
    for call in calls {
        let name = call.get("name").and_then(Value::as_str).unwrap_or("");
        let mut error = String::new();
        let args = match serde_json::from_str::<Value>(
            call.get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}"),
        ) {
            Ok(value) => value,
            Err(e) => {
                error.push_str(&format!("Error parsing tool call arguments: {e}."));
                json!({})
            }
        };
        if name != "bash" {
            error.push_str(&format!("Unknown tool '{name}'."));
        }
        if !args.is_object() || args.get("command").is_none() {
            error.push_str("Missing 'command' argument in bash tool call.");
        }
        if !error.is_empty() {
            let text = render_error(
                format_error_template,
                error.trim().into(),
                vec![],
                true,
                finish_reason,
            )?;
            return Err(MiniError::new(
                ErrorKind::Format,
                json!({"type":"message","role":"user","content":[{"type":"input_text","text":text}],"extra":{"interrupt_type":"FormatError","has_tool_calls":true}}),
            ));
        }
        actions.push(json!({"command":args["command"],"tool_call_id":call.get("call_id").or_else(||call.get("id"))}));
    }
    Ok(actions)
}

pub fn response_finish_reason(response: &Value) -> Option<Value> {
    let status = response.get("status").and_then(Value::as_str);
    if status == Some("incomplete") {
        let reason = response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str);
        Some(json!(if reason == Some("max_output_tokens") {
            "length"
        } else {
            "incomplete"
        }))
    } else {
        response.get("status").cloned()
    }
}

pub fn expand_multimodal(content: Json, pattern: &str) -> Result<Json> {
    fn expand(value: Json, re: &regex::Regex) -> Json {
        match value {
            Value::String(text) => {
                let mut result = Vec::new();
                let mut last = 0;
                let mut found = false;
                for captures in re.captures_iter(&text) {
                    found = true;
                    let whole = captures.get(0).unwrap();
                    if whole.start() > last {
                        result.push(json!({"type":"text","text":&text[last..whole.start()]}));
                    }
                    let kind = captures.get(1).map_or("", |m| m.as_str().trim());
                    let extracted = captures.get(2).map_or("", |m| m.as_str().trim());
                    if kind == "image_url" {
                        result.push(json!({"type":"image_url","image_url":{"url":extracted}}));
                    }
                    last = whole.end();
                }
                if found && last < text.len() {
                    result.push(json!({"type":"text","text":&text[last..]}));
                }
                if !found {
                    result.push(json!({"type":"text","text":text}));
                }
                json!(result)
            }
            Value::Array(items) => Value::Array(items.into_iter().map(|v| expand(v, re)).collect()),
            Value::Object(mut object) => {
                if let Some(content) = object.remove("content") {
                    object.insert("content".into(), expand(content, re));
                }
                Value::Object(object)
            }
            other => Value::String(other.to_string()),
        }
    }
    if pattern.is_empty() {
        return Ok(content);
    }
    Ok(expand(content, &regex::Regex::new(pattern)?))
}

pub fn reorder_thinking_blocks(messages: Vec<Json>) -> Vec<Json> {
    messages
        .into_iter()
        .map(|mut message| {
            if message.get("role").and_then(Value::as_str) == Some("assistant") {
                if let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) {
                    let mut thinking = Vec::new();
                    let mut other = Vec::new();
                    for block in content.drain(..) {
                        if matches!(
                            block.get("type").and_then(Value::as_str),
                            Some("thinking" | "redacted_thinking")
                        ) {
                            thinking.push(block);
                        } else {
                            other.push(block);
                        }
                    }
                    if !thinking.is_empty() {
                        if other.is_empty() {
                            other.push(json!({"type":"text","text":""}));
                        }
                        thinking.extend(other);
                        *content = thinking;
                    }
                }
            }
            message
        })
        .collect()
}

pub fn set_cache_control(mut messages: Vec<Json>, mode: Option<&str>) -> Result<Vec<Json>> {
    let Some(mode) = mode else {
        return Ok(messages);
    };
    if mode != "default_end" {
        return Err(crate::error::value_error(format!("Invalid mode: {mode}")));
    }
    let previous_end = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.get("role").and_then(Value::as_str) == Some("assistant"))
        .map(|(i, _)| i as i64)
        .next_back()
        .unwrap_or(0)
        - 1;
    let last_index = messages.len() - 1;
    for (index, message) in messages.iter_mut().enumerate() {
        let is_end = index == last_index;
        clear_cache(message);
        if index as i64 == previous_end || is_end {
            set_cache(message);
        }
    }
    Ok(messages)
}

fn clear_cache(message: &mut Json) {
    if let Some(object) = message.as_object_mut() {
        object.remove("cache_control");
        if let Some(content) = object.get_mut("content").and_then(Value::as_array_mut) {
            if let Some(first) = content.first_mut().and_then(Value::as_object_mut) {
                first.remove("cache_control");
            }
        }
    }
}

fn set_cache(message: &mut Json) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    if object.get("content").is_none_or(Value::is_null)
        || object.get("role").and_then(Value::as_str) == Some("tool")
    {
        object.insert("cache_control".into(), json!({"type":"ephemeral"}));
        return;
    }
    match object.get_mut("content") {
        Some(Value::String(text)) => {
            let text = std::mem::take(text);
            object.insert(
                "content".into(),
                json!([{"type":"text","text":text,"cache_control":{"type":"ephemeral"}}]),
            );
        }
        Some(Value::Array(content)) => {
            if let Some(first) = content.first_mut().and_then(Value::as_object_mut) {
                first.insert("cache_control".into(), json!({"type":"ephemeral"}));
            }
        }
        _ => {}
    }
}

pub fn expand_multimodal_content(content: Json, pattern: &str) -> Result<Json> {
    expand_multimodal(content, pattern)
}
pub fn parse_toolcall_actions_response(
    output: &[Json],
    format_error_template: &str,
    template_kwargs: Option<&Map>,
) -> Result<Vec<Json>> {
    parse_response_actions(
        output,
        format_error_template,
        template_kwargs.and_then(|m| m.get("finish_reason")),
    )
}

pub fn prepare_messages(messages: &[Json], cache_mode: Option<&str>) -> Result<Vec<Json>> {
    let prepared = messages.iter().map(without_extra).collect();
    set_cache_control(reorder_thinking_blocks(prepared), cache_mode)
}

pub fn prepare_response_messages(messages: &[Json]) -> Vec<Json> {
    let mut result = Vec::new();
    for message in messages {
        if message.get("object").and_then(Value::as_str) == Some("response") {
            if let Some(Value::Array(items)) = message.get("output") {
                result.extend(items.iter().map(without_extra));
            }
        } else {
            result.push(without_extra(message));
        }
    }
    result
}
