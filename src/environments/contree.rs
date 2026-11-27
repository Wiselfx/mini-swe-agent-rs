use super::{
    Environment,
    output::{CommandOutput, check_finished},
    serialized_environment, system_template_vars,
};
use crate::{Json, Map, Result, config::with_defaults, error::value_error};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    env, fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

const TYPE_NAME: &str = "minisweagent.environments.extra.contree.ContreeEnvironment";
const DEFAULT_URL: &str = "https://api.tokenfactory.nebius.com/sandboxes";

pub struct ContreeEnvironment {
    pub config: Map,
    client: reqwest::blocking::Client,
    base_url: String,
    token: String,
    project: Option<String>,
    image_uuid: String,
}

#[derive(Clone)]
struct Connection {
    url: String,
    token: String,
    project: Option<String>,
}

impl ContreeEnvironment {
    pub fn new(mut config: Map) -> Result<Self> {
        config = with_defaults(
            config,
            json!({
                "image_tag": null, "cwd": "/", "cwd_auto_create": true, "env": {}, "forward_env": [],
                "interpreter": ["bash", "-c"], "timeout": 100, "import_username": null, "import_password": null
            }),
        );
        let image = config
            .get("image")
            .and_then(Value::as_str)
            .ok_or_else(|| value_error("ConTree environment requires 'image'"))?
            .to_owned();
        let connection = resolve_connection(config.get("contree_config"))?;
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()?;
        let mut env = Self {
            config,
            client,
            base_url: connection.url,
            token: connection.token,
            project: connection.project,
            image_uuid: String::new(),
        };
        env.image_uuid = env.import_image(&image)?;
        if env
            .config
            .get("cwd_auto_create")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            let cwd = env
                .config
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or("/")
                .to_owned();
            let command = format!("mkdir -p {}", shell_quote(&cwd));
            let output = env.run(&command, "/", 100)?;
            if output.returncode != 0 {
                return Err(value_error(output.output));
            }
        }
        Ok(env)
    }

    fn request(&self, method: reqwest::Method, path: &str, body: Option<Json>) -> Result<Json> {
        let mut request = self
            .client
            .request(
                method,
                format!("{}/v1{}", self.base_url.trim_end_matches('/'), path),
            )
            .bearer_auth(&self.token);
        if let Some(project) = &self.project {
            request = request.header("Project", project);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send()?;
        let status = response.status();
        let text = response.text()?;
        if !status.is_success() {
            return Err(value_error(format!(
                "ConTree HTTP {}: {text}",
                status.as_u16()
            )));
        }
        if text.trim().is_empty() {
            Ok(json!({}))
        } else {
            serde_json::from_str(&text).map_err(Into::into)
        }
    }

    fn import_image(&self, image: &str) -> Result<String> {
        let tag = self.config.get("image_tag").and_then(Value::as_str);
        let mut registry = Map::from_iter([("url".into(), json!(normalize_registry(image)))]);
        let username = self.config.get("import_username").and_then(Value::as_str);
        let password = self.config.get("import_password").and_then(Value::as_str);
        if let (Some(username), Some(password)) = (username, password) {
            registry.insert(
                "credentials".into(),
                json!({"username":username,"password":password}),
            );
        }
        let mut body = Map::from_iter([
            ("registry".into(), Value::Object(registry)),
            ("timeout".into(), json!(300)),
        ]);
        if let Some(tag) = tag {
            body.insert("tag".into(), json!(tag));
        }
        let response = self.request(
            reqwest::Method::POST,
            "/images/import",
            Some(Value::Object(body)),
        )?;
        let operation = response
            .get("uuid")
            .and_then(Value::as_str)
            .ok_or_else(|| value_error("ConTree import returned no operation UUID"))?;
        let response = self.wait_operation(operation, 300)?;
        response
            .get("result_image_uuid")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                value_error(format!(
                    "ConTree image import failed: {}",
                    response
                        .get("error")
                        .cloned()
                        .unwrap_or(json!("unknown error"))
                ))
            })
    }

    fn wait_operation(&self, id: &str, timeout: u64) -> Result<Json> {
        let start = Instant::now();
        loop {
            let response =
                self.request(reqwest::Method::GET, &format!("/operations/{id}"), None)?;
            match response.get("status").and_then(Value::as_str).unwrap_or("") {
                "SUCCESS" | "FAILED" | "CANCELLED" => return Ok(response),
                _ => {}
            }
            if start.elapsed().as_secs() >= timeout {
                return Err(value_error(format!(
                    "ConTree operation {id} timed out after {timeout} seconds"
                )));
            }
            thread::sleep(Duration::from_millis(500));
        }
    }

    fn run(&mut self, command: &str, cwd: &str, timeout: u64) -> Result<CommandOutput> {
        let mut vars = HashMap::new();
        for key in self
            .config
            .get("forward_env")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Ok(value) = env::var(key) {
                vars.insert(key.to_owned(), value);
            }
        }
        for (key, value) in super::string_map(&self.config, "env") {
            vars.insert(key, value);
        }
        let interpreter = self
            .config
            .get("interpreter")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| "bash -c".into());
        let body = json!({
            "command": format!("{interpreter} {}", shell_quote(command)), "image": self.image_uuid,
            "shell": true, "cwd": cwd, "timeout": timeout, "disposable": false,
            "env": if vars.is_empty() { Value::Null } else { json!(vars) }
        });
        let spawned = self.request(reqwest::Method::POST, "/instances", Some(body))?;
        let id = spawned
            .get("uuid")
            .and_then(Value::as_str)
            .ok_or_else(|| value_error("ConTree returned no operation UUID"))?;
        let response = self.wait_operation(id, timeout + 5)?;
        if let Some(image) = response.get("result_image_uuid").and_then(Value::as_str) {
            self.image_uuid = image.to_owned();
        }
        let result = response.pointer("/metadata/result").ok_or_else(|| {
            value_error(format!(
                "ConTree operation failed: {}",
                response
                    .get("error")
                    .cloned()
                    .unwrap_or(json!("unknown error"))
            ))
        })?;
        let stdout = stream_text(result.get("stdout"));
        let stderr = stream_text(result.get("stderr"));
        let code = result
            .pointer("/state/exit_code")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        Ok(CommandOutput::success(format!("{stdout}{stderr}"), code))
    }
}

impl Environment for ContreeEnvironment {
    fn execute(&mut self, action: &Json, cwd: &str, timeout: Option<u64>) -> Result<CommandOutput> {
        let workdir = if cwd.is_empty() {
            self.config
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or("/")
                .to_owned()
        } else {
            cwd.to_owned()
        };
        let timeout = timeout.unwrap_or_else(|| {
            self.config
                .get("timeout")
                .and_then(Value::as_u64)
                .unwrap_or(100)
        });
        let result = self.run(&super::action_command(action), &workdir, timeout);
        let output = match result {
            Ok(output) => output,
            Err(error) => CommandOutput::failure(String::new(), error, "ContreeError"),
        };
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

fn stream_text(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if value.get("encoding").and_then(Value::as_str) == Some("base64") {
        use base64::Engine;
        return String::from_utf8_lossy(
            &base64::engine::general_purpose::STANDARD
                .decode(value.get("value").and_then(Value::as_str).unwrap_or(""))
                .unwrap_or_default(),
        )
        .into_owned();
    }
    value
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn shell_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c))
    {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn normalize_registry(image: &str) -> String {
    if image.starts_with("docker://") {
        return image.to_owned();
    }
    let parts: Vec<_> = image.split('/').collect();
    let (registry, path) = if parts.len() == 1 {
        ("docker.io", format!("library/{}", parts[0]))
    } else if parts[0].contains('.') || parts[0].contains(':') {
        (parts[0], parts[1..].join("/"))
    } else {
        ("docker.io", parts.join("/"))
    };
    let path = if path.rsplit('/').next().is_some_and(|s| !s.contains(':')) {
        format!("{path}:latest")
    } else {
        path
    };
    format!("docker://{registry}/{path}")
}

fn resolve_connection(config: Option<&Value>) -> Result<Connection> {
    let config = config
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let profile = config.get("profile").and_then(Value::as_str);
    let config_path = config
        .get("config_path")
        .and_then(Value::as_str)
        .map(|s| s.to_owned());
    let mut url = config.get("url").and_then(Value::as_str).map(str::to_owned);
    let mut token = config
        .get("token")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut project = config
        .get("project")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut auth_type = config
        .get("auth_type")
        .and_then(Value::as_str)
        .unwrap_or("iam")
        .to_owned();
    if url.is_none()
        || token.is_none()
        || project.is_none()
        || profile.is_some()
        || config_path.is_some()
    {
        let home = env::var("CONTREE_HOME")
            .map(PathBuf::from)
            .or_else(|_| env::var("XDG_CONFIG_HOME").map(|p| PathBuf::from(p).join("contree")))
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".config/contree"));
        let auth = config_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("auth.ini"));
        let text = [home.join("cli.ini"), auth]
            .iter()
            .filter_map(|p| fs::read_to_string(p).ok())
            .collect::<Vec<_>>()
            .join("\n");
        let selected = profile
            .map(str::to_owned)
            .or_else(|| env::var("CONTREE_PROFILE").ok())
            .unwrap_or_else(|| "default".into());
        let mut in_profile = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_profile = line == format!("[profile:{selected}]");
                continue;
            }
            if !in_profile || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                match key.trim() {
                    "url" => {
                        url.get_or_insert_with(|| value.trim().to_owned());
                    }
                    "token" => {
                        token.get_or_insert_with(|| value.trim().to_owned());
                    }
                    "project" => {
                        project.get_or_insert_with(|| value.trim().to_owned());
                    }
                    "type" => auth_type = value.trim().to_owned(),
                    _ => {}
                }
            }
        }
    }
    if token.is_none() {
        token = env::var("CONTREE_TOKEN")
            .ok()
            .or_else(|| env::var("NEBIUS_API_KEY").ok());
    }
    if url.is_none() {
        url = env::var("CONTREE_URL").ok();
    }
    if project.is_none() {
        project = env::var("CONTREE_PROJECT")
            .ok()
            .or_else(|| env::var("NEBIUS_AI_PROJECT").ok());
    }
    let token = token.filter(|s| !s.is_empty()).ok_or_else(|| {
        value_error("ConTree token is required (CONTREE_TOKEN or contree_config.token)")
    })?;
    let url = url
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if auth_type == "iam" {
                Some(DEFAULT_URL.into())
            } else {
                None
            }
        })
        .ok_or_else(|| value_error("ConTree URL is required for JWT authentication"))?;
    Ok(Connection {
        url,
        token,
        project,
    })
}
