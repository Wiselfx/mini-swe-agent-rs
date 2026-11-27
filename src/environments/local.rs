use super::{
    Environment,
    output::{CommandOutput, check_finished},
    run_command, serialized_environment, string_map, system_template_vars, timeout_value,
};
use crate::{Json, Map, Result, config::with_defaults};
use serde_json::json;
use std::{collections::HashMap, env, path::PathBuf};

pub const TYPE_NAME: &str = "minisweagent.environments.local.LocalEnvironment";

pub struct LocalEnvironment {
    pub config: Map,
}

impl LocalEnvironment {
    pub fn new(config: Map) -> Self {
        Self {
            config: with_defaults(config, json!({"cwd": "", "env": {}, "timeout": 30})),
        }
    }
}

impl Environment for LocalEnvironment {
    fn execute(&mut self, action: &Json, cwd: &str, timeout: Option<u64>) -> Result<CommandOutput> {
        let command = super::action_command(action);
        let current_dir;
        let workdir = if cwd.is_empty() {
            self.config.get("cwd").and_then(Json::as_str).unwrap_or("")
        } else {
            cwd
        };
        let workdir = if workdir.is_empty() {
            current_dir = env::current_dir()?.to_string_lossy().into_owned();
            current_dir.as_str()
        } else {
            workdir
        };
        let mut vars: HashMap<String, String> = env::vars().collect();
        vars.extend(string_map(&self.config, "env"));
        #[cfg(unix)]
        let output = run_command(
            "/bin/sh",
            &["-c".into(), command.into()],
            PathBuf::from(workdir).as_path(),
            &vars,
            timeout_value(&self.config, timeout),
            true,
        );
        #[cfg(windows)]
        let output = run_command(
            &env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()),
            &["/C".into(), command.into()],
            PathBuf::from(workdir).as_path(),
            &vars,
            timeout_value(&self.config, timeout),
            false,
        );
        check_finished(&output)?;
        Ok(output)
    }
    fn get_template_vars(&self) -> Map {
        let mut vars = self.config.clone();
        vars.extend(system_template_vars());
        vars.extend(env::vars().map(|(k, v)| (k, json!(v))).collect::<Map>());
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
}

pub fn _type_check(_: Option<Json>) -> Option<Json> {
    None
}
