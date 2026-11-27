use super::{
    Environment,
    output::{CommandOutput, check_finished},
    serialized_environment,
};
use crate::{Json, Map, Result, config::with_defaults, error::value_error};
use serde_json::{Value, json};
use std::time::Duration;

pub const TYPE_NAME: &str = "minisweagent.environments.extra.swerex_modal.SwerexModalEnvironment";

pub struct SwerexModalEnvironment {
    pub config: Map,
    client: reqwest::blocking::Client,
    endpoint: String,
    token: Option<String>,
}

impl SwerexModalEnvironment {
    pub fn new(config: Map) -> Result<Self> {
        let config = with_defaults(
            config,
            json!({
                "cwd": "/", "timeout": 30, "env": {}, "startup_timeout": 60.0,
                "runtime_timeout": 3600.0, "deployment_timeout": 3600.0,
                "install_pipx": true, "modal_sandbox_kwargs": {}
            }),
        );
        let endpoint = config.get("runtime_url").and_then(Value::as_str).map(str::to_owned)
            .or_else(||std::env::var("SWEREX_URL").ok()).ok_or_else(||value_error("Native Modal sandbox provisioning is unavailable; set runtime_url or SWEREX_URL to an existing SWE-ReX server"))?;
        let token = config
            .get("auth_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| std::env::var("SWEREX_AUTH_TOKEN").ok());
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        let env = Self {
            config,
            client,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            token,
        };
        env.check_alive()?;
        Ok(env)
    }

    fn check_alive(&self) -> Result<()> {
        let mut request = self.client.get(format!("{}/is_alive", self.endpoint));
        if let Some(token) = &self.token {
            request = request.header("X-API-Key", token);
        }
        let response: Json = request.send()?.error_for_status()?.json()?;
        if response.get("is_alive").and_then(Value::as_bool) != Some(true) {
            return Err(value_error(format!(
                "SWE-ReX runtime is not alive: {}",
                response
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            )));
        }
        Ok(())
    }
}

impl Environment for SwerexModalEnvironment {
    fn execute(&mut self, action: &Json, cwd: &str, timeout: Option<u64>) -> Result<CommandOutput> {
        let workdir = if cwd.is_empty() {
            self.config
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or("/")
        } else {
            cwd
        };
        let timeout = timeout.unwrap_or_else(|| {
            self.config
                .get("timeout")
                .and_then(Value::as_u64)
                .unwrap_or(30)
        });
        let env_vars = self
            .config
            .get("env")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let body = json!({"command":super::action_command(action),"shell":true,"check":false,"cwd":workdir,"timeout":timeout,"merge_output_streams":true,"env":if env_vars.is_empty(){Value::Null}else{Value::Object(env_vars)}});
        let mut request = self
            .client
            .post(format!("{}/execute", self.endpoint))
            .json(&body)
            .header("X-Request-ID", uuid::Uuid::new_v4().to_string());
        if let Some(token) = &self.token {
            request = request.header("X-API-Key", token);
        }
        let result = (|| -> Result<CommandOutput> {
            let response: Json = request.send()?.error_for_status()?.json()?;
            Ok(CommandOutput::success(
                response
                    .get("stdout")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                response
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .unwrap_or(-1),
            ))
        })();
        let output = match result {
            Ok(output) => output,
            Err(error) => CommandOutput::failure(String::new(), error, "SwerexError"),
        };
        check_finished(&output)?;
        Ok(output)
    }
    fn get_template_vars(&self) -> Map {
        self.config.clone()
    }
    fn serialize(&self) -> Json {
        serialized_environment(TYPE_NAME, &self.config)
    }
    fn type_name(&self) -> &str {
        TYPE_NAME
    }
    fn config(&self) -> &Map {
        &self.config
    }
    fn config_mut(&mut self) -> &mut Map {
        &mut self.config
    }
}
