use crate::{
    Json, Map, Result,
    config::with_defaults,
    environments::CommandOutput,
    error::value_error,
    models::{
        Model,
        actions::{
            format_observation_messages, format_toolcall_observation_messages_impl, timestamp,
        },
        serialized_model, stats,
    },
};
use serde_json::{Value, json};
use std::{thread, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeterministicStyle {
    Text,
    Toolcall,
    Response,
}

pub struct DeterministicModel {
    pub style: DeterministicStyle,
    pub config: Map,
    current_index: isize,
}

impl DeterministicModel {
    pub fn new(style: DeterministicStyle, config: Map) -> Self {
        let model_name = match style {
            DeterministicStyle::Text => "deterministic",
            DeterministicStyle::Toolcall => "deterministic_toolcall",
            DeterministicStyle::Response => "deterministic_response_api_toolcall",
        };
        let defaults = json!({
            "model_name": model_name, "cost_per_call": 1.0,
            "observation_template": "{% if output.exception_info %}<exception>{{output.exception_info}}</exception>\n{% endif %}<returncode>{{output.returncode}}</returncode>\n<output>\n{{output.output}}</output>",
            "multimodal_regex": ""
        });
        Self {
            style,
            config: with_defaults(config, defaults),
            current_index: -1,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self.style {
            DeterministicStyle::Text => "minisweagent.models.test_models.DeterministicModel",
            DeterministicStyle::Toolcall => {
                "minisweagent.models.test_models.DeterministicToolcallModel"
            }
            DeterministicStyle::Response => {
                "minisweagent.models.test_models.DeterministicResponseAPIToolcallModel"
            }
        }
    }
}

impl Model for DeterministicModel {
    fn query(&mut self, _messages: &[Json]) -> Result<Json> {
        loop {
            self.current_index += 1;
            let index = usize::try_from(self.current_index)
                .map_err(|_| value_error("Deterministic model outputs exhausted"))?;
            let output = self
                .config
                .get("outputs")
                .and_then(Value::as_array)
                .and_then(|a| a.get(index))
                .cloned()
                .ok_or_else(|| value_error("Deterministic model outputs exhausted"))?;
            let actions = output
                .pointer("/extra/actions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut retry = false;
            for action in actions {
                if let Some(raise) = action.get("raise") {
                    return Err(value_error(raise.to_string()));
                }
                let command = action.get("command").and_then(Value::as_str).unwrap_or("");
                if let Some(seconds) = command.strip_prefix("/sleep ") {
                    thread::sleep(Duration::from_secs_f64(seconds.parse().unwrap_or(0.0)));
                    retry = true;
                }
                if command.starts_with("/warning") {
                    crate::run::logging::warning(&command[7..]);
                    retry = true;
                }
            }
            if retry {
                continue;
            }
            let cost = self
                .config
                .get("cost_per_call")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            stats::add(cost)?;
            return Ok(output);
        }
    }

    fn format_message(&self, message: Json) -> Json {
        if self.style == DeterministicStyle::Response {
            let mut object = message.as_object().cloned().unwrap_or_default();
            object.insert("type".into(), json!("message"));
            object
                .entry("role".to_owned())
                .or_insert_with(|| json!("user"));
            if let Some(Value::String(text)) = object.get("content").cloned() {
                object.insert("content".into(), json!([{"type":"input_text","text":text}]));
            }
            Value::Object(object)
        } else {
            let pattern = self
                .config
                .get("multimodal_regex")
                .and_then(Value::as_str)
                .unwrap_or("");
            crate::models::actions::expand_multimodal(message, pattern)
                .unwrap_or_else(|_| json!({}))
        }
    }

    fn format_observation_messages(
        &self,
        message: &Json,
        outputs: &[CommandOutput],
        vars: &Map,
    ) -> Result<Vec<Json>> {
        let template = self
            .config
            .get("observation_template")
            .and_then(Value::as_str)
            .unwrap_or(default_observation());
        let regex = self
            .config
            .get("multimodal_regex")
            .and_then(Value::as_str)
            .unwrap_or("");
        match self.style {
            DeterministicStyle::Text => format_observation_messages(outputs, template, vars, regex),
            DeterministicStyle::Toolcall | DeterministicStyle::Response => {
                let actions = message
                    .pointer("/extra/actions")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                format_toolcall_observation_messages_impl(
                    actions,
                    outputs,
                    template,
                    vars,
                    regex,
                    self.style == DeterministicStyle::Response,
                )
            }
        }
    }

    fn get_template_vars(&self) -> Map {
        self.config.clone()
    }
    fn serialize(&self) -> Json {
        serialized_model(self.type_name(), &self.config)
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

fn default_observation() -> &'static str {
    "{% if output.exception_info %}<exception>{{output.exception_info}}</exception>\n{% endif %}<returncode>{{output.returncode}}</returncode>\n<output>\n{{output.output}}</output>"
}

pub fn _timestamp_type(_: Option<f64>) {
    let _ = timestamp();
}
