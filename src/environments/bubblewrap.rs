use super::{
    Environment,
    output::{CommandOutput, check_finished},
    run_command, serialized_environment, string_list, string_map, system_template_vars,
    timeout_value,
};
use crate::{Json, Map, Result, config::with_defaults};
use serde_json::json;
use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};

pub const TYPE_NAME: &str = "minisweagent.environments.extra.bubblewrap.BubblewrapEnvironment";

pub struct BubblewrapEnvironment {
    pub config: Map,
    pub working_dir: PathBuf,
}

impl BubblewrapEnvironment {
    pub fn new(config: Map) -> Result<Self> {
        let config = with_defaults(
            config,
            json!({
                "cwd": "", "env": {}, "timeout": 30,
                "executable": env::var("MSWEA_BUBBLEWRAP_EXECUTABLE").unwrap_or_else(|_| "bwrap".into()),
                "wrapper_args": ["--unshare-user-try", "--ro-bind", "/usr", "/usr", "--ro-bind", "/bin", "/bin", "--ro-bind", "/lib", "/lib", "--ro-bind", "/lib64", "/lib64", "--ro-bind", "/etc", "/etc", "--tmpfs", "/tmp", "--proc", "/proc", "--dev", "/dev", "--new-session", "--setenv", "PATH", "/usr/local/bin:/usr/sbin:/usr/bin:/bin"]
            }),
        );
        let working_dir = env::temp_dir().join(format!(
            "minisweagent-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        std::fs::create_dir_all(&working_dir)?;
        Ok(Self {
            config,
            working_dir,
        })
    }
}

impl Environment for BubblewrapEnvironment {
    fn execute(&mut self, action: &Json, cwd: &str, timeout: Option<u64>) -> Result<CommandOutput> {
        let default_dir = self.working_dir.to_string_lossy().into_owned();
        let workdir = if cwd.is_empty() {
            self.config
                .get("cwd")
                .and_then(Json::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(&default_dir)
        } else {
            cwd
        };
        let executable = self
            .config
            .get("executable")
            .and_then(Json::as_str)
            .unwrap_or("bwrap");
        let mut args = string_list(&self.config, "wrapper_args");
        args.extend([
            "--bind".into(),
            workdir.into(),
            workdir.into(),
            "--chdir".into(),
            workdir.into(),
        ]);
        for (key, value) in string_map(&self.config, "env") {
            args.extend(["--setenv".into(), key.into(), value.into()]);
        }
        args.extend([
            "bash".into(),
            "-c".into(),
            super::action_command(action).into(),
        ]);
        let output = run_command(
            executable,
            &args,
            Path::new("."),
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
}

impl Drop for BubblewrapEnvironment {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.working_dir);
    }
}
