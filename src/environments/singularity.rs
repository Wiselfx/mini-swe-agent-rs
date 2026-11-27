use super::{
    Environment,
    output::{CommandOutput, check_finished},
    run_command, serialized_environment, string_list, string_map, timeout_value,
};
use crate::{Json, Map, Result, config::with_defaults, error::value_error};
use serde_json::json;
use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    process::Command,
};

pub const TYPE_NAME: &str = "minisweagent.environments.singularity.SingularityEnvironment";

pub struct SingularityEnvironment {
    pub config: Map,
    pub sandbox_dir: Option<PathBuf>,
}

impl SingularityEnvironment {
    pub fn new(config: Map) -> Result<Self> {
        let config = with_defaults(
            config,
            json!({
                "cwd": "/", "env": {}, "forward_env": [], "timeout": 30,
                "executable": env::var("MSWEA_SINGULARITY_EXECUTABLE").unwrap_or_else(|_| "singularity".into()),
                "sandbox_build_retries": 3, "global_args": ["--quiet"],
                "exec_args": ["--contain", "--cleanenv", "--fakeroot"]
            }),
        );
        let mut env = Self {
            config,
            sandbox_dir: None,
        };
        env.build_sandbox()?;
        Ok(env)
    }

    fn executable(&self) -> &str {
        self.config
            .get("executable")
            .and_then(Json::as_str)
            .unwrap_or("singularity")
    }

    fn build_sandbox(&mut self) -> Result<()> {
        let image = self
            .config
            .get("image")
            .and_then(Json::as_str)
            .ok_or_else(|| value_error("Singularity environment requires 'image'"))?;
        let retries = self
            .config
            .get("sandbox_build_retries")
            .and_then(Json::as_u64)
            .unwrap_or(3)
            .max(1);
        let mut last = None;
        for _ in 0..retries {
            let dir = env::temp_dir().join(format!(
                "minisweagent-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            ));
            let output = Command::new(self.executable())
                .args(["build", "--sandbox"])
                .arg(&dir)
                .arg(image)
                .output();
            match output {
                Ok(output) if output.status.success() => {
                    self.sandbox_dir = Some(dir);
                    return Ok(());
                }
                Ok(output) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    last = Some(format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    last = Some(error.to_string());
                }
            }
        }
        Err(value_error(format!(
            "Error building image {image}: {}",
            last.unwrap_or_default()
        )))
    }
}

impl Environment for SingularityEnvironment {
    fn execute(&mut self, action: &Json, cwd: &str, timeout: Option<u64>) -> Result<CommandOutput> {
        let sandbox = self
            .sandbox_dir
            .as_ref()
            .ok_or_else(|| value_error("Sandbox not built"))?;
        let mut args = string_list(&self.config, "global_args");
        args.push("exec".into());
        args.extend(string_list(&self.config, "exec_args"));
        let workdir = if cwd.is_empty() {
            self.config.get("cwd").and_then(Json::as_str).unwrap_or("/")
        } else {
            cwd
        };
        if !workdir.is_empty() && workdir != "/" {
            args.extend(["--pwd".into(), workdir.into()]);
        }
        for key in self
            .config
            .get("forward_env")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
        {
            if let Ok(value) = env::var(key) {
                args.extend(["--env".into(), format!("{key}={value}").into()]);
            }
        }
        for (key, value) in string_map(&self.config, "env") {
            args.extend(["--env".into(), format!("{key}={value}").into()]);
        }
        args.extend([
            "--writable".into(),
            sandbox.as_os_str().to_owned(),
            "bash".into(),
            "-c".into(),
            super::action_command(action).into(),
        ]);
        let output = run_command(
            self.executable(),
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

impl Drop for SingularityEnvironment {
    fn drop(&mut self) {
        if let Some(dir) = &self.sandbox_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
