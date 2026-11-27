use crate::{
    Json, Map, Result,
    config::with_defaults,
    environments::CommandOutput,
    error::value_error,
    models::{
        Model,
        actions::{
            bash_tool, bash_tool_response, expand_multimodal, format_observation_messages,
            format_toolcall_observation_messages, format_toolcall_observation_messages_impl,
            parse_regex_actions, parse_response_actions, parse_toolcall_actions, prepare_messages,
            prepare_response_messages, response_finish_reason, timestamp,
        },
        registry::{self, model_info_with_custom},
        serialized_model, stats,
    },
};
use serde_json::{Value, json};
use std::{env, thread, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    LiteLLM,
    OpenRouter,
    Portkey,
    Requesty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Toolcall,
    Textbased,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    OpenAi,
    Anthropic,
}

pub struct NativeModel {
    pub provider: Provider,
    pub style: Style,
    pub config: Map,
    pub type_name: String,
    client: reqwest::blocking::Client,
    custom_registry: Json,
}

fn default_observation() -> &'static str {
    "{% if output.exception_info %}<exception>{{output.exception_info}}</exception>\n{% endif %}<returncode>{{output.returncode}}</returncode>\n<output>\n{{output.output}}</output>"
}

pub fn model_defaults(provider: Provider, style: Style, model_name: &str) -> Json {
    let cost_tracking = env::var("MSWEA_COST_TRACKING").unwrap_or_else(|_| "default".into());
    let mut config = serde_json::Map::new();
    config.insert("model_name".into(), json!(model_name));
    config.insert("model_kwargs".into(), json!({}));
    config.insert("set_cache_control".into(), Value::Null);
    config.insert("cost_tracking".into(), json!(cost_tracking));
    config.insert("format_error_template".into(), json!("{{ error }}"));
    config.insert("observation_template".into(), json!(default_observation()));
    config.insert("multimodal_regex".into(), json!(""));
    if provider == Provider::LiteLLM || provider == Provider::Portkey {
        config.insert(
            "litellm_model_registry".into(),
            env::var("LITELLM_MODEL_REGISTRY_PATH")
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
    }
    if provider == Provider::LiteLLM {
        config.insert("context_window".into(), Value::Null);
        config.insert("thinking_levels".into(), Value::Null);
        config.insert("pricing".into(), Value::Null);
        config.insert("model_profiles".into(), json!({}));
    }
    if provider == Provider::Portkey {
        config.insert("provider".into(), json!(""));
        config.insert("litellm_model_name_override".into(), json!(""));
    }
    if style == Style::Textbased {
        config.insert(
            "action_regex".into(),
            json!(r"```mswea_bash_command\s*\n(.*?)\n```"),
        );
        config.insert("format_error_template".into(), json!("Please always provide EXACTLY ONE action in triple backticks, found {{actions|length}} actions."));
    }
    Value::Object(config)
}

impl NativeModel {
    pub fn new(
        provider: Provider,
        style: Style,
        mut config: Map,
        type_name: impl Into<String>,
    ) -> Result<Self> {
        let model_name = config
            .get("model_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        config = with_defaults(config, model_defaults(provider, style, &model_name));
        let custom_registry = config
            .get("litellm_model_registry")
            .and_then(Value::as_str)
            .filter(|p| std::path::Path::new(p).is_file())
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<Json>(&text).ok())
            .unwrap_or_else(|| json!({}));
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        if provider == Provider::Portkey && env::var("PORTKEY_API_KEY").is_err() {
            return Err(value_error(
                "Portkey API key is required. Set PORTKEY_API_KEY or run `mini-extra config set PORTKEY_API_KEY YOUR_KEY`.",
            ));
        }
        Ok(Self {
            provider,
            style,
            config,
            type_name: type_name.into(),
            client,
            custom_registry,
        })
    }

    pub fn type_name_for(provider: Provider, style: Style) -> &'static str {
        match (provider, style) {
            (Provider::LiteLLM, Style::Toolcall) => {
                "minisweagent.models.litellm_model.LitellmModel"
            }
            (Provider::LiteLLM, Style::Textbased) => {
                "minisweagent.models.litellm_textbased_model.LitellmTextbasedModel"
            }
            (Provider::LiteLLM, Style::Response) => {
                "minisweagent.models.litellm_response_model.LitellmResponseModel"
            }
            (Provider::OpenRouter, Style::Toolcall) => {
                "minisweagent.models.openrouter_model.OpenRouterModel"
            }
            (Provider::OpenRouter, Style::Textbased) => {
                "minisweagent.models.openrouter_textbased_model.OpenRouterTextbasedModel"
            }
            (Provider::OpenRouter, Style::Response) => {
                "minisweagent.models.openrouter_response_model.OpenRouterResponseModel"
            }
            (Provider::Portkey, Style::Toolcall) => {
                "minisweagent.models.portkey_model.PortkeyModel"
            }
            (Provider::Portkey, Style::Response) => {
                "minisweagent.models.portkey_response_model.PortkeyResponseAPIModel"
            }
            (Provider::Requesty, Style::Toolcall) => {
                "minisweagent.models.requesty_model.RequestyModel"
            }
            _ => "minisweagent.models.litellm_model.LitellmModel",
        }
    }

    fn model_name(&self) -> &str {
        self.config
            .get("model_name")
            .and_then(Value::as_str)
            .unwrap_or("")
    }
    fn info(&self) -> Option<&Map> {
        model_info_with_custom(self.model_name(), Some(&self.custom_registry))
    }
    fn cost_info(&self) -> Option<&Map> {
        self.config
            .get("litellm_model_name_override")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .and_then(|name| model_info_with_custom(name, Some(&self.custom_registry)))
            .or_else(|| self.info())
    }
    fn cache_mode(&self) -> Option<&str> {
        self.config.get("set_cache_control").and_then(Value::as_str)
    }
    fn observation_template(&self) -> &str {
        self.config
            .get("observation_template")
            .and_then(Value::as_str)
            .unwrap_or(default_observation())
    }
    fn format_error_template(&self) -> &str {
        self.config
            .get("format_error_template")
            .and_then(Value::as_str)
            .unwrap_or("{{ error }}")
    }
    fn multimodal_regex(&self) -> &str {
        self.config
            .get("multimodal_regex")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    fn endpoint(&self) -> Result<(String, Transport, String)> {
        let kwargs = self.config.get("model_kwargs").and_then(Value::as_object);
        if let Some(base) = kwargs
            .and_then(|m| m.get("api_base"))
            .and_then(Value::as_str)
        {
            return Ok((
                format!(
                    "{}/{}",
                    base.trim_end_matches('/'),
                    if self.style == Style::Response {
                        "responses"
                    } else {
                        "chat/completions"
                    }
                ),
                Transport::OpenAi,
                self.model_name().to_owned(),
            ));
        }
        match self.provider {
            Provider::OpenRouter => Ok((
                format!(
                    "https://openrouter.ai/api/v1/{}",
                    if self.style == Style::Response {
                        "responses"
                    } else {
                        "chat/completions"
                    }
                ),
                Transport::OpenAi,
                self.model_name()
                    .trim_start_matches("openrouter/")
                    .to_owned(),
            )),
            Provider::Requesty => Ok((
                "https://router.requesty.ai/v1/chat/completions".into(),
                Transport::OpenAi,
                self.model_name().to_owned(),
            )),
            Provider::Portkey => Ok((
                format!(
                    "{}/{}",
                    env::var("PORTKEY_BASE_URL")
                        .unwrap_or_else(|_| "https://api.portkey.ai/v1".into())
                        .trim_end_matches('/'),
                    if self.style == Style::Response {
                        "responses"
                    } else {
                        "chat/completions"
                    }
                ),
                Transport::OpenAi,
                self.model_name().to_owned(),
            )),
            Provider::LiteLLM => {
                let name = self.model_name();
                if let Some(model) = name.strip_prefix("anthropic/") {
                    let base = env::var("ANTHROPIC_BASE_URL")
                        .unwrap_or_else(|_| "https://api.anthropic.com/v1".into());
                    return Ok((
                        format!("{}/messages", base.trim_end_matches('/')),
                        Transport::Anthropic,
                        model.to_owned(),
                    ));
                }
                if let Some(model) = name.strip_prefix("openrouter/") {
                    return Ok((
                        format!(
                            "https://openrouter.ai/api/v1/{}",
                            if self.style == Style::Response {
                                "responses"
                            } else {
                                "chat/completions"
                            }
                        ),
                        Transport::OpenAi,
                        model.to_owned(),
                    ));
                }
                let (base, model) = if let Some(model) = name.strip_prefix("openai/") {
                    (
                        env::var("OPENAI_BASE_URL")
                            .or_else(|_| env::var("OPENAI_API_BASE"))
                            .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
                        model.to_owned(),
                    )
                } else if let Some(model) = name.strip_prefix("gemini/") {
                    (
                        env::var("GEMINI_BASE_URL")
                            .or_else(|_| env::var("GEMINI_API_BASE"))
                            .unwrap_or_else(|_| {
                                "https://generativelanguage.googleapis.com/v1beta/openai".into()
                            }),
                        model.to_owned(),
                    )
                } else if let Some(model) = name.strip_prefix("groq/") {
                    (
                        env::var("GROQ_BASE_URL")
                            .unwrap_or_else(|_| "https://api.groq.com/openai/v1".into()),
                        model.to_owned(),
                    )
                } else if let Some(model) = name.strip_prefix("deepseek/") {
                    (
                        env::var("DEEPSEEK_BASE_URL")
                            .unwrap_or_else(|_| "https://api.deepseek.com/v1".into()),
                        model.to_owned(),
                    )
                } else if let Some(model) = name.strip_prefix("mistral/") {
                    (
                        env::var("MISTRAL_BASE_URL")
                            .unwrap_or_else(|_| "https://api.mistral.ai/v1".into()),
                        model.to_owned(),
                    )
                } else if let Some(model) = name.strip_prefix("xai/") {
                    (
                        env::var("XAI_BASE_URL").unwrap_or_else(|_| "https://api.x.ai/v1".into()),
                        model.to_owned(),
                    )
                } else if let Some(model) = name
                    .strip_prefix("together_ai/")
                    .or_else(|| name.strip_prefix("together/"))
                {
                    (
                        env::var("TOGETHER_BASE_URL")
                            .unwrap_or_else(|_| "https://api.together.xyz/v1".into()),
                        model.to_owned(),
                    )
                } else if let Some(model) = name.strip_prefix("fireworks_ai/") {
                    (
                        env::var("FIREWORKS_BASE_URL")
                            .unwrap_or_else(|_| "https://api.fireworks.ai/inference/v1".into()),
                        model.to_owned(),
                    )
                } else if let Ok(base) =
                    env::var("OPENAI_BASE_URL").or_else(|_| env::var("OPENAI_API_BASE"))
                {
                    (base, name.to_owned())
                } else {
                    return Err(value_error(format!(
                        "No native Rust API route for model '{name}'. Set model_kwargs.api_base or use an OpenAI-compatible MSWEA model name."
                    )));
                };
                Ok((
                    format!(
                        "{}/{}",
                        base.trim_end_matches('/'),
                        if self.style == Style::Response {
                            "responses"
                        } else {
                            "chat/completions"
                        }
                    ),
                    Transport::OpenAi,
                    model,
                ))
            }
        }
    }

    fn request_builder(&self, url: &str, body: &Json) -> reqwest::blocking::RequestBuilder {
        let mut builder = self.client.post(url).json(body);
        let name = self.model_name().to_lowercase();
        let key_name = if self.provider == Provider::OpenRouter || name.starts_with("openrouter/") {
            "OPENROUTER_API_KEY"
        } else if self.provider == Provider::Requesty {
            "REQUESTY_API_KEY"
        } else if self.provider == Provider::Portkey {
            "PORTKEY_API_KEY"
        } else if name.starts_with("anthropic/") {
            "ANTHROPIC_API_KEY"
        } else if name.starts_with("gemini/") {
            "GEMINI_API_KEY"
        } else if name.starts_with("groq/") {
            "GROQ_API_KEY"
        } else if name.starts_with("deepseek/") {
            "DEEPSEEK_API_KEY"
        } else if name.starts_with("mistral/") {
            "MISTRAL_API_KEY"
        } else if name.starts_with("xai/") {
            "XAI_API_KEY"
        } else if name.starts_with("together") {
            "TOGETHER_API_KEY"
        } else if name.starts_with("fireworks") {
            "FIREWORKS_API_KEY"
        } else {
            "OPENAI_API_KEY"
        };
        if let Ok(key) = env::var(key_name) {
            if self.provider == Provider::Portkey {
                builder = builder.header("x-portkey-api-key", key);
            } else if name.starts_with("anthropic/") {
                builder = builder
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01");
            } else {
                builder = builder.bearer_auth(key);
            }
        }
        if self.provider == Provider::Portkey {
            if let Ok(key) = env::var("PORTKEY_VIRTUAL_KEY") {
                builder = builder.header("x-portkey-virtual-key", key);
            } else if let Some(provider) = self
                .config
                .get("provider")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                builder = builder.header("x-portkey-provider", provider);
            }
        }
        if self.provider == Provider::Requesty {
            builder = builder
                .header(
                    "HTTP-Referer",
                    "https://github.com/SWE-agent/mini-swe-agent",
                )
                .header("X-Title", "mini-swe-agent");
        }
        builder
    }

    fn payload_kwargs(&self) -> Map {
        self.config
            .get("model_kwargs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    fn query_once(&self, messages: &[Json]) -> Result<Json> {
        let (url, transport, api_model) = self.endpoint()?;
        let mut kwargs = self.payload_kwargs();
        let api_base = kwargs.remove("api_base");
        let _ = api_base;
        kwargs.remove("drop_params");
        kwargs.remove("custom_llm_provider");
        let prepared = if self.style == Style::Response {
            prepare_response_messages(messages)
        } else {
            prepare_messages(messages, self.cache_mode())?
        };
        let body = if transport == Transport::Anthropic {
            anthropic_request(
                &api_model,
                &prepared,
                if self.style == Style::Response {
                    bash_tool_response()
                } else {
                    bash_tool()
                },
                &kwargs,
            )?
        } else if self.style == Style::Response {
            let mut body = Map::from_iter([
                ("model".into(), json!(api_model)),
                ("input".into(), json!(prepared)),
                ("tools".into(), json!([bash_tool_response()])),
            ]);
            body.extend(kwargs);
            Value::Object(body)
        } else {
            let mut body = Map::from_iter([
                ("model".into(), json!(api_model)),
                ("messages".into(), json!(prepared)),
            ]);
            if self.style == Style::Toolcall {
                body.insert("tools".into(), json!([bash_tool()]));
            }
            if self.provider == Provider::OpenRouter {
                body.insert("usage".into(), json!({"include":true}));
            }
            body.extend(kwargs);
            Value::Object(body)
        };
        let response = self
            .request_builder(&url, &body)
            .send()
            .map_err(|e| value_error(format!("Request failed: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|e| value_error(format!("Request failed: {e}")))?;
        if !status.is_success() {
            let hint = if status.as_u16() == 401 {
                " You can permanently set your API key with `mini-extra config set KEY VALUE`."
            } else {
                ""
            };
            return Err(value_error(format!(
                "HTTP {}: {text}{hint}",
                status.as_u16()
            )));
        }
        let mut response: Json = serde_json::from_str(&text)
            .map_err(|e| value_error(format!("Invalid JSON response: {e}: {text}")))?;
        if transport == Transport::Anthropic {
            response = anthropic_response_to_openai(response);
        }
        Ok(response)
    }

    fn query_with_retry(&self, messages: &[Json]) -> Result<Json> {
        let attempts = env::var("MSWEA_MODEL_RETRY_STOP_AFTER_ATTEMPT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(10)
            .max(1);
        let mut last = None;
        for attempt in 0..attempts {
            match self.query_once(messages) {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let text = error.to_string();
                    if text.contains("HTTP 401")
                        || text.contains("HTTP 404")
                        || text.contains("HTTP 403")
                        || text.contains("context") && text.contains("exceed")
                    {
                        return Err(error);
                    }
                    if attempt + 1 == attempts {
                        return Err(error);
                    }
                    let wait = (4u64 * 2u64.pow((attempt.min(4)) as u32).min(60)).max(4);
                    thread::sleep(Duration::from_secs(wait));
                    last = Some(error);
                }
            }
        }
        Err(last.unwrap_or_else(|| value_error("Model query failed")))
    }

    fn calculate_cost(&self, response: &Json) -> Result<f64> {
        let tracking = self
            .config
            .get("cost_tracking")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let cost = if self.provider == Provider::OpenRouter {
            response
                .pointer("/usage/cost")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
        } else if self.provider == Provider::Requesty {
            let cost = response
                .pointer("/usage/cost")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            if cost == 0.0 {
                return Err(value_error(format!(
                    "No cost information available from Requesty API for model {}. Cost tracking is required but not provided by the API response.",
                    self.model_name()
                )));
            }
            cost
        } else {
            let usage = registry::usage(response);
            let mut prompt = usage
                .get("prompt_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let completion = usage
                .get("completion_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if self.provider == Provider::Portkey {
                let total = response
                    .pointer("/usage/total_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(prompt + completion);
                if total - prompt - completion != 0 {
                    prompt = total - completion;
                }
            }
            let cached = usage
                .get("cached_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let written = usage
                .get("cache_write_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let mut cost = None;
            if let Some(prices) = self.config.get("pricing").and_then(Value::as_object) {
                let get = |key: &str| prices.get(key).and_then(Value::as_f64).unwrap_or(0.0) / 1e6;
                cost = Some(
                    (prompt - cached) as f64 * get("input")
                        + cached as f64 * get("cache_read")
                        + completion as f64 * get("output")
                        + written as f64 * get("cache_write"),
                );
            } else if prompt > 0 {
                if let Some(info) = self.cost_info() {
                    if let (Some(input), Some(output)) = (
                        info.get("input_cost_per_token").and_then(Value::as_f64),
                        info.get("output_cost_per_token").and_then(Value::as_f64),
                    ) {
                        cost = Some(
                            (prompt - cached) as f64 * input
                                + cached as f64
                                    * info
                                        .get("cache_read_input_token_cost")
                                        .and_then(Value::as_f64)
                                        .unwrap_or(0.0)
                                + completion as f64 * output
                                + written as f64
                                    * info
                                        .get("cache_creation_input_token_cost")
                                        .and_then(Value::as_f64)
                                        .unwrap_or(0.0),
                        );
                    }
                }
            }
            cost.unwrap_or(0.0)
        };
        if cost <= 0.0 && !(self.provider == Provider::OpenRouter && tracking == "ignore_errors") {
            if tracking == "ignore_errors" {
                return Ok(0.0);
            }
            return Err(value_error(format!(
                "Error calculating cost for model {}: Cost must be > 0.0, got {cost}. Perhaps it's not registered? Set cost_tracking: 'ignore_errors' or MSWEA_COST_TRACKING='ignore_errors'.",
                self.model_name()
            )));
        }
        Ok(cost)
    }

    fn parse_actions(&self, response: &Json) -> Result<Vec<Json>> {
        let finish = response.pointer("/choices/0/finish_reason");
        match self.style {
            Style::Toolcall => {
                let calls = response
                    .pointer("/choices/0/message/tool_calls")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                parse_toolcall_actions(calls, self.format_error_template(), finish)
            }
            Style::Textbased => {
                let content = response
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let regex = self
                    .config
                    .get("action_regex")
                    .and_then(Value::as_str)
                    .unwrap_or(r"```mswea_bash_command\s*\n(.*?)\n```");
                parse_regex_actions(content, regex, self.format_error_template(), finish)
            }
            Style::Response => {
                let output = response
                    .get("output")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                parse_response_actions(
                    output,
                    self.format_error_template(),
                    response_finish_reason(response).as_ref(),
                )
            }
        }
    }

    fn assistant_message(&self, response: &Json) -> Json {
        if self.style == Style::Response {
            response.clone()
        } else {
            response
                .pointer("/choices/0/message")
                .cloned()
                .unwrap_or_else(|| json!({}))
        }
    }
}

impl Model for NativeModel {
    fn query(&mut self, messages: &[Json]) -> Result<Json> {
        let response = self.query_with_retry(messages)?;
        let cost = self.calculate_cost(&response)?;
        stats::add(cost)?;
        let usage = registry::usage(&response);
        match self.parse_actions(&response) {
            Ok(actions) => {
                let mut message = self.assistant_message(&response);
                let mut extra = Map::new();
                extra.insert("actions".into(), json!(actions));
                if self.style != Style::Response
                    && matches!(self.provider, Provider::LiteLLM | Provider::Portkey)
                {
                    extra.insert("response".into(), response.clone());
                }
                if self.provider == Provider::LiteLLM {
                    extra.insert("usage".into(), json!(usage));
                }
                extra.insert("cost".into(), json!(cost));
                extra.insert("timestamp".into(), json!(timestamp()));
                if let Some(object) = message.as_object_mut() {
                    object.insert("extra".into(), Value::Object(extra));
                }
                Ok(message)
            }
            Err(mut error) => {
                let mut extras = Map::from_iter([
                    ("cost".into(), json!(cost)),
                    ("response".into(), response.clone()),
                ]);
                if self.provider == Provider::LiteLLM {
                    extras.insert("usage".into(), json!(usage));
                }
                if let Some(first) = error.messages.first_mut().and_then(Value::as_object_mut) {
                    let old = first
                        .remove("extra")
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                    old.into_iter().for_each(|(k, v)| {
                        extras.insert(k, v);
                    });
                    first.insert("extra".into(), Value::Object(extras));
                }
                if self.style == Style::Response {
                    error.messages.insert(0, response);
                } else {
                    error.messages.insert(0, self.assistant_message(&response));
                }
                Err(error)
            }
        }
    }

    fn format_message(&self, message: Json) -> Json {
        if self.style == Style::Response {
            let mut object = message.as_object().cloned().unwrap_or_default();
            object.insert("type".into(), json!("message"));
            object
                .entry("role".to_owned())
                .or_insert_with(|| json!("user"));
            if let Some(Value::String(text)) = object.get("content").cloned() {
                object.insert("content".into(), json!([{"type":"input_text","text":text}]));
            }
            if object.get("extra").is_none_or(|v| v.is_null()) {
                object.remove("extra");
            }
            Value::Object(object)
        } else {
            expand_multimodal(message.clone(), self.multimodal_regex()).unwrap_or(message)
        }
    }

    fn format_observation_messages(
        &self,
        message: &Json,
        outputs: &[CommandOutput],
        template_vars: &Map,
    ) -> Result<Vec<Json>> {
        match self.style {
            Style::Textbased => format_observation_messages(
                outputs,
                self.observation_template(),
                template_vars,
                self.multimodal_regex(),
            ),
            Style::Toolcall => {
                let actions = message
                    .get("extra")
                    .and_then(|e| e.get("actions"))
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                format_toolcall_observation_messages(
                    actions,
                    outputs,
                    self.observation_template(),
                    template_vars,
                    self.multimodal_regex(),
                )
            }
            Style::Response => {
                let actions = message
                    .get("extra")
                    .and_then(|e| e.get("actions"))
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                format_toolcall_observation_messages_impl(
                    actions,
                    outputs,
                    self.observation_template(),
                    template_vars,
                    "",
                    true,
                )
            }
        }
    }

    fn get_template_vars(&self) -> Map {
        self.config.clone()
    }
    fn serialize(&self) -> Json {
        serialized_model(&self.type_name, &self.config)
    }
    fn get_context_window(&self) -> Option<u64> {
        self.config
            .get("context_window")
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
            .or_else(|| {
                self.info()
                    .and_then(|i| i.get("max_input_tokens"))
                    .and_then(Value::as_u64)
                    .filter(|v| *v > 0)
            })
    }
    fn get_thinking_levels(&self) -> Option<Vec<String>> {
        if let Some(levels) = self.config.get("thinking_levels").and_then(Value::as_array) {
            let mut seen = Vec::new();
            for level in levels
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
            {
                if !seen.contains(&level) {
                    seen.push(level);
                }
            }
            return Some(seen);
        }
        let info = self.info()?;
        const ORDER: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
        if let Some(levels) = info
            .get("reasoning_effort_levels")
            .and_then(Value::as_array)
            .filter(|v| !v.is_empty())
        {
            let set: std::collections::HashSet<_> = levels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_lowercase)
                .collect();
            return Some(
                ORDER
                    .iter()
                    .rev()
                    .filter(|l| set.contains(**l))
                    .map(|s| s.to_string())
                    .collect(),
            );
        }
        let supported: std::collections::HashSet<_> = ORDER
            .iter()
            .copied()
            .filter(
                |level| match info.get(&format!("supports_{level}_reasoning_effort")) {
                    Some(Value::Bool(value)) => *value,
                    Some(Value::String(value)) => matches!(value.as_str(), "true" | "True"),
                    _ => false,
                },
            )
            .collect();
        if !supported.is_empty() {
            return Some(
                ORDER
                    .iter()
                    .rev()
                    .filter(|l| supported.contains(*l))
                    .map(|s| s.to_string())
                    .collect(),
            );
        }
        if info.get("supports_reasoning") == Some(&Value::Bool(false)) {
            Some(vec![])
        } else {
            None
        }
    }
    fn apply_profile(&mut self, name: &str) -> Result<String> {
        let profiles = self
            .config
            .get("model_profiles")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| value_error("This model does not support profiles"))?;
        let profile = profiles
            .get(name)
            .cloned()
            .ok_or_else(|| value_error(format!("Unknown model profile '{name}'")))?;
        for field in [
            "model_name",
            "model_kwargs",
            "cost_tracking",
            "pricing",
            "thinking_levels",
            "context_window",
        ] {
            self.config.remove(field);
        }
        let defaults = model_defaults(self.provider, self.style, "");
        if let Some(object) = defaults.as_object() {
            for (key, value) in object {
                self.config.entry(key.clone()).or_insert(value.clone());
            }
        }
        let profile = profile
            .as_object()
            .ok_or_else(|| value_error("Model profile must be an object"))?;
        for (field, value) in profile {
            if !model_defaults(self.provider, self.style, "")
                .as_object()
                .unwrap()
                .contains_key(field)
            {
                return Err(value_error(format!(
                    "Unknown field '{field}' in model profile '{name}'"
                )));
            }
            self.config.insert(field.clone(), value.clone());
        }
        let levels = self.get_thinking_levels();
        if let Some(kwargs) = self
            .config
            .get_mut("model_kwargs")
            .and_then(Value::as_object_mut)
        {
            if levels.as_ref().is_some_and(Vec::is_empty) {
                kwargs.remove("reasoning_effort");
            } else if let Some(levels) = levels {
                if !levels.contains(
                    &kwargs
                        .get("reasoning_effort")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                ) {
                    kwargs.insert(
                        "reasoning_effort".into(),
                        json!(if levels.iter().any(|l| l == "high") {
                            "high".to_owned()
                        } else {
                            levels[0].clone()
                        }),
                    );
                }
            }
        }
        Ok(self.model_name().to_owned())
    }
    fn config(&self) -> &Map {
        &self.config
    }
    fn config_mut(&mut self) -> &mut Map {
        &mut self.config
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn anthropic_request(model: &str, messages: &[Json], _tool: Json, kwargs: &Map) -> Result<Json> {
    let mut system = Vec::new();
    let mut converted = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        if role == "system" {
            if let Some(text) = message.get("content").and_then(Value::as_str) {
                system.push(json!({"type":"text","text":text}));
            }
            continue;
        }
        if role == "tool" {
            let block = json!({"type":"tool_result","tool_use_id":message.get("tool_call_id"),"content":message.get("content").cloned().unwrap_or_else(||json!("")),"cache_control":message.get("cache_control")});
            if converted
                .last()
                .and_then(|m: &Json| m.get("role"))
                .and_then(Value::as_str)
                == Some("user")
            {
                converted
                    .last_mut()
                    .unwrap()
                    .get_mut("content")
                    .unwrap()
                    .as_array_mut()
                    .unwrap()
                    .push(block);
            } else {
                converted.push(json!({"role":"user","content":[block]}));
            }
            continue;
        }
        let mut blocks = Vec::new();
        match message.get("content") {
            Some(Value::String(text)) => blocks.push(json!({"type":"text","text":text})),
            Some(Value::Array(items)) => {
                for item in items {
                    match item.get("type").and_then(Value::as_str) {
                        Some("text") => blocks.push(item.clone()),
                        Some("image_url") => {
                            let url = item
                                .pointer("/image_url/url")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            if url.starts_with("data:") {
                                let (header, data) =
                                    url.split_once(',').unwrap_or(("data:image/png;base64", ""));
                                let media = header
                                    .strip_prefix("data:")
                                    .and_then(|s| s.split(';').next())
                                    .unwrap_or("image/png");
                                blocks.push(json!({"type":"image","source":{"type":"base64","media_type":media,"data":data}}));
                            } else {
                                blocks.push(
                                    json!({"type":"image","source":{"type":"url","url":url}}),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if role == "assistant" {
            if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    let function = call.get("function").unwrap_or(&Value::Null);
                    let input = serde_json::from_str(
                        function
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}"),
                    )
                    .unwrap_or_else(|_| json!({}));
                    blocks.push(json!({"type":"tool_use","id":call.get("id"),"name":function.get("name"),"input":input}));
                }
            }
        }
        converted.push(json!({"role":role,"content":blocks}));
    }
    let mut body = Map::from_iter([
        ("model".into(), json!(model)),
        ("messages".into(), json!(converted)),
        (
            "max_tokens".into(),
            json!(
                kwargs
                    .get("max_tokens")
                    .cloned()
                    .unwrap_or_else(|| json!(8192))
            ),
        ),
        (
            "tools".into(),
            json!([{"name":"bash","description":"Execute a bash command","input_schema":{"type":"object","properties":{"command":{"type":"string","description":"The bash command to execute"}},"required":["command"]}}]),
        ),
    ]);
    if !system.is_empty() {
        body.insert("system".into(), json!(system));
    }
    for (key, value) in kwargs {
        if !matches!(key.as_str(), "drop_params" | "api_base" | "model") {
            body.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(body))
}

fn anthropic_response_to_openai(response: Json) -> Json {
    let mut content = Vec::new();
    let mut calls = Vec::new();
    let mut reasoning = String::new();
    if let Some(blocks) = response.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => content.push(json!({"type":"text","text":block.get("text")})),
                Some("thinking") => reasoning.push_str(block.get("thinking").and_then(Value::as_str).unwrap_or("")),
                Some("tool_use") => calls.push(json!({"id":block.get("id"),"type":"function","function":{"name":block.get("name"),"arguments":block.get("input").unwrap_or(&json!({})).to_string()}})),
                _ => {}
            }
        }
    }
    let input = response
        .pointer("/usage/input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = response
        .pointer("/usage/output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut message = Map::from_iter([
        ("role".into(), json!("assistant")),
        ("content".into(), json!(content)),
    ]);
    if !reasoning.is_empty() {
        message.insert("reasoning_content".into(), json!(reasoning));
    }
    if !calls.is_empty() {
        message.insert("tool_calls".into(), json!(calls));
    }
    json!({"choices":[{"message":Value::Object(message),"finish_reason":if calls.is_empty(){"stop"}else{"tool_calls"}}],"usage":{"prompt_tokens":input,"completion_tokens":output,"cache_read_input_tokens":response.pointer("/usage/cache_read_input_tokens").cloned().unwrap_or_else(||json!(0)),"cache_creation_input_tokens":response.pointer("/usage/cache_creation_input_tokens").cloned().unwrap_or_else(||json!(0))}})
}
