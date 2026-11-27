use super::{
    Environment,
    output::{CommandOutput, check_finished},
    run_command, serialized_environment, string_list, string_map, system_template_vars,
    timeout_value,
};
use crate::{Json, Map, Result, config::with_defaults};
use serde_json::json;
use std::{collections::HashMap, env, ffi::OsString, path::PathBuf, process::Command};

pub const TYPE_NAME: &str = "minisweagent.environments.docker.DockerEnvironment";

pub struct DockerEnvironment {
    pub config: Map,
    pub container_id: Option<String>,
}

impl DockerEnvironment {
    pub fn new(config: Map) -> Result<Self> {
        let config = with_defaults(
            config,
            json!({
                "cwd": "/", "env": {}, "forward_env": [], "timeout": 30,
                "executable": env::var("MSWEA_DOCKER_EXECUTABLE").unwrap_or_else(|_| "docker".into()),
                "run_args": ["--rm"], "container_timeout": "2h", "pull_timeout": 120,
                "interpreter": ["bash", "-lc"]
            }),
        );
        let mut env = Self {
            config,
            container_id: None,
        };
        env.start_container()?;
        Ok(env)
    }

    fn executable(&self) -> &str {
        self.config
            .get("executable")
            .and_then(Json::as_str)
            .unwrap_or("docker")
    }

    fn start_container(&mut self) -> Result<()> {
        let image = self
            .config
            .get("image")
            .and_then(Json::as_str)
            .ok_or_else(|| crate::error::value_error("Docker environment requires 'image'"))?;
        let name = format!(
            "minisweagent-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let cwd = self.config.get("cwd").and_then(Json::as_str).unwrap_or("/");
        let mut args: Vec<OsString> = [
            "run".into(),
            "-d".into(),
            "--name".into(),
            name.as_str().into(),
            "-w".into(),
            cwd.into(),
        ]
        .into_iter()
        .collect();
        args.extend(string_list(&self.config, "run_args"));
        args.extend([
            image.into(),
            "sleep".into(),
            self.config
                .get("container_timeout")
                .and_then(Json::as_str)
                .unwrap_or("2h")
                .into(),
        ]);
        let output = Command::new(self.executable()).args(&args).output();
        match output {
            Ok(output) if output.status.success() => {
                self.container_id = Some(String::from_utf8_lossy(&output.stdout).trim().to_owned());
                Ok(())
            }
            Ok(output) => Err(crate::error::value_error(format!(
                "Failed to start container: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ))),
            Err(error) => Err(error.into()),
        }
    }
}

impl Environment for DockerEnvironment {
    fn execute(&mut self, action: &Json, cwd: &str, timeout: Option<u64>) -> Result<CommandOutput> {
        let id = self
            .container_id
            .as_ref()
            .ok_or_else(|| crate::error::value_error("Container not started"))?;
        let workdir = if cwd.is_empty() {
            self.config.get("cwd").and_then(Json::as_str).unwrap_or("/")
        } else {
            cwd
        };
        let mut args: Vec<OsString> = ["exec".into(), "-w".into(), workdir.into()]
            .into_iter()
            .collect();
        for key in self
            .config
            .get("forward_env")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
        {
            if let Ok(value) = env::var(key) {
                args.extend(["-e".into(), format!("{key}={value}").into()]);
            }
        }
        for (key, value) in string_map(&self.config, "env") {
            args.extend(["-e".into(), format!("{key}={value}").into()]);
        }
        args.push(id.clone().into());
        args.extend(string_list(&self.config, "interpreter"));
        args.push(super::action_command(action).into());
        let output = run_command(
            self.executable(),
            &args,
            &PathBuf::from("."),
            &HashMap::new(),
            timeout_value(&self.config, timeout),
            false,
        );
        check_finished(&output)?;
        Ok(output)
    }
    fn get_template_vars(&self) -> Map {
        let mut vars = self.config.clone();
        vars.extend(system_template_vars());
        vars
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
    fn container_id(&self) -> Option<&str> {
        self.container_id.as_deref()
    }
    fn executable(&self) -> Option<&str> {
        self.config.get("executable").and_then(Json::as_str)
    }
}

impl Drop for DockerEnvironment {
    fn drop(&mut self) {
        if let Some(id) = &self.container_id {
            let _ = Command::new(self.executable())
                .args(["rm", "-f", id])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
}
