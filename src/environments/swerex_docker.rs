use super::{DockerEnvironment, Environment, docker::TYPE_NAME as DOCKER_TYPE_NAME};
use crate::{Json, Map, Result, config::with_defaults, environments::CommandOutput};
use serde_json::json;

pub const TYPE_NAME: &str = "minisweagent.environments.extra.swerex_docker.SwerexDockerEnvironment";

pub struct SwerexDockerEnvironment {
    inner: DockerEnvironment,
    original_config: Map,
}

impl SwerexDockerEnvironment {
    pub fn new(config: Map) -> Result<Self> {
        let original_config = with_defaults(
            config.clone(),
            json!({"cwd":"/","timeout":30,"deployment_extra_kwargs":{}}),
        );
        let mut docker_config = config;
        docker_config.remove("deployment_extra_kwargs");
        if let Some(extra) = original_config
            .get("deployment_extra_kwargs")
            .and_then(Json::as_object)
        {
            if let Some(runtime) = extra.get("container_runtime").and_then(Json::as_str) {
                docker_config.insert("executable".into(), json!(runtime));
            }
            if let Some(args) = extra.get("docker_args").and_then(Json::as_array) {
                docker_config.insert("run_args".into(), json!(args));
            }
        }
        Ok(Self {
            inner: DockerEnvironment::new(docker_config)?,
            original_config,
        })
    }
}

impl Environment for SwerexDockerEnvironment {
    fn execute(&mut self, action: &Json, cwd: &str, timeout: Option<u64>) -> Result<CommandOutput> {
        self.inner.execute(action, cwd, timeout)
    }
    fn get_template_vars(&self) -> Map {
        self.original_config.clone()
    }
    fn serialize(&self) -> Json {
        super::serialized_environment(TYPE_NAME, &self.original_config)
    }
    fn type_name(&self) -> &str {
        TYPE_NAME
    }
    fn config(&self) -> &Map {
        &self.original_config
    }
    fn config_mut(&mut self) -> &mut Map {
        &mut self.original_config
    }
    fn container_id(&self) -> Option<&str> {
        self.inner.container_id()
    }
    fn executable(&self) -> Option<&str> {
        self.inner.executable()
    }
}

pub const _DOCKER_TYPE: &str = DOCKER_TYPE_NAME;
