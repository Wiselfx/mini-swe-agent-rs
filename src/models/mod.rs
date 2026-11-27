pub mod actions;
mod deterministic;
mod native;
mod registry;
mod roulette;
pub mod stats;

pub use crate::content::get_content_string;
pub use actions::{
    bash_tool as BASH_TOOL, bash_tool_response as BASH_TOOL_RESPONSE_API,
    expand_multimodal_content, format_observation_messages, format_toolcall_observation_messages,
    parse_regex_actions, parse_toolcall_actions, parse_toolcall_actions_response, prepare_messages,
    set_cache_control,
};
pub use deterministic::{DeterministicModel, DeterministicStyle};
pub use native::{NativeModel, Provider, Style};
pub use roulette::RouletteModel;

use crate::{Json, Map, Result, environments::CommandOutput, error::value_error};
use serde_json::Value;
use std::{any::Any, env};

pub trait Model: Send {
    fn query(&mut self, messages: &[Json]) -> Result<Json>;
    fn format_message(&self, message: Json) -> Json;
    fn format_observation_messages(
        &self,
        message: &Json,
        outputs: &[CommandOutput],
        template_vars: &Map,
    ) -> Result<Vec<Json>>;
    fn get_template_vars(&self) -> Map;
    fn serialize(&self) -> Json;
    fn get_context_window(&self) -> Option<u64> {
        None
    }
    fn get_thinking_levels(&self) -> Option<Vec<String>> {
        None
    }
    fn apply_profile(&mut self, _name: &str) -> Result<String> {
        Err(value_error("This model does not support profiles"))
    }
    fn config(&self) -> &Map;
    fn config_mut(&mut self) -> &mut Map;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

pub fn serialized_model(type_name: &str, config: &Map) -> Json {
    serde_json::json!({"info":{"config":{"model":config,"model_type":type_name}}})
}

pub fn get_model_name(input: Option<&str>, config: &Map) -> Result<String> {
    if let Some(name) = input.filter(|s| !s.is_empty()) {
        return Ok(name.to_owned());
    }
    if let Some(name) = config
        .get("model_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Ok(name.to_owned());
    }
    if let Ok(name) = env::var("MSWEA_MODEL_NAME") {
        if !name.is_empty() {
            return Ok(name);
        }
    }
    Err(value_error(
        "No default model set. Please run `mini-extra config setup` to set one.",
    ))
}

pub fn get_model(config: Map) -> Result<Box<dyn Model>> {
    get_model_with_name(None, &config)
}

pub fn get_model_with_name(input: Option<&str>, config: &Map) -> Result<Box<dyn Model>> {
    let mut config = config.clone();
    let name = get_model_name(input, &config)?;
    config.insert("model_name".into(), Json::String(name.clone()));
    let spec = config
        .remove("model_class")
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let class = spec.rsplit('.').next().unwrap_or("litellm");
    if class.contains("Roulette") || spec == "roulette" {
        return Ok(Box::new(RouletteModel::new(config, false)?));
    }
    if class.contains("Interleaving") || spec == "interleaving" {
        return Ok(Box::new(RouletteModel::new(config, true)?));
    }
    if class.contains("DeterministicResponse") || spec == "deterministic_response_api_toolcall" {
        return Ok(Box::new(DeterministicModel::new(
            DeterministicStyle::Response,
            config,
        )));
    }
    if class.contains("DeterministicToolcall") || spec == "deterministic_toolcall" {
        return Ok(Box::new(DeterministicModel::new(
            DeterministicStyle::Toolcall,
            config,
        )));
    }
    if class.contains("Deterministic") || spec == "deterministic" {
        return Ok(Box::new(DeterministicModel::new(
            DeterministicStyle::Text,
            config,
        )));
    }
    let provider = if class.contains("OpenRouter") || spec == "openrouter" {
        Provider::OpenRouter
    } else if class.contains("Portkey") || spec == "portkey" {
        Provider::Portkey
    } else if class.contains("Requesty") || spec == "requesty" {
        Provider::Requesty
    } else if class.contains("Litellm") || spec == "litellm" || spec.is_empty() {
        Provider::LiteLLM
    } else {
        return Err(value_error(format!("Unknown model class: {spec}")));
    };
    let style = if class.contains("Textbased") || spec.ends_with("_textbased") {
        Style::Textbased
    } else if class.contains("Response") || spec.ends_with("_response") {
        Style::Response
    } else {
        Style::Toolcall
    };
    let type_name = NativeModel::type_name_for(provider, style);
    if provider == Provider::LiteLLM
        && name.to_lowercase().contains("anthropic")
        && !config.contains_key("set_cache_control")
    {
        config.insert(
            "set_cache_control".into(),
            Json::String("default_end".into()),
        );
    }
    Ok(Box::new(NativeModel::new(
        provider, style, config, type_name,
    )?))
}
