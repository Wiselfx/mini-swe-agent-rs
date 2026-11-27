use crate::{
    Json, Map, Result,
    config::with_defaults,
    environments::CommandOutput,
    error::value_error,
    models::{Model, get_model, serialized_model},
};
use serde_json::{Value, json};

pub struct RouletteModel {
    pub config: Map,
    pub models: Vec<Box<dyn Model>>,
    pub sequence: Option<Vec<usize>>,
    pub interleaving: bool,
    n_calls: usize,
}

impl RouletteModel {
    pub fn new(config: Map, interleaving: bool) -> Result<Self> {
        let defaults = json!({"model_name":if interleaving{"interleaving"}else{"roulette"}});
        let config = with_defaults(config, defaults);
        let entries = config
            .get("model_kwargs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut models = Vec::new();
        for entry in entries {
            let object = entry
                .as_object()
                .cloned()
                .ok_or_else(|| value_error("model_kwargs entries must be objects"))?;
            models.push(get_model(object)?);
        }
        if models.is_empty() {
            return Err(value_error("RouletteModel requires at least one model"));
        }
        let sequence = config.get("sequence").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .map(|v| v as usize)
                .collect()
        });
        Ok(Self {
            config,
            models,
            sequence,
            interleaving,
            n_calls: 0,
        })
    }

    fn selected(&self) -> &dyn Model {
        let index = if self.interleaving {
            self.sequence
                .as_ref()
                .map(|s| s[self.n_calls % s.len()])
                .unwrap_or(self.n_calls % self.models.len())
        } else {
            rand::random_range(0..self.models.len())
        };
        self.models[index].as_ref()
    }
}

impl Model for RouletteModel {
    fn query(&mut self, messages: &[Json]) -> Result<Json> {
        let index = if self.interleaving {
            self.sequence
                .as_ref()
                .map(|s| s[self.n_calls % s.len()])
                .unwrap_or(self.n_calls % self.models.len())
        } else {
            rand::random_range(0..self.models.len())
        };
        self.n_calls += 1;
        let mut response = self.models[index].query(messages)?;
        let name = self.models[index]
            .config()
            .get("model_name")
            .cloned()
            .unwrap_or(json!("unknown"));
        if let Some(object) = response.as_object_mut() {
            object.insert("model_name".into(), name);
        }
        Ok(response)
    }
    fn format_message(&self, message: Json) -> Json {
        self.selected().format_message(message)
    }
    fn format_observation_messages(
        &self,
        message: &Json,
        outputs: &[CommandOutput],
        vars: &Map,
    ) -> Result<Vec<Json>> {
        self.selected()
            .format_observation_messages(message, outputs, vars)
    }
    fn get_template_vars(&self) -> Map {
        self.config.clone()
    }
    fn serialize(&self) -> Json {
        serialized_model(
            if self.interleaving {
                "minisweagent.models.extra.roulette.InterleavingModel"
            } else {
                "minisweagent.models.extra.roulette.RouletteModel"
            },
            &self.config,
        )
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
