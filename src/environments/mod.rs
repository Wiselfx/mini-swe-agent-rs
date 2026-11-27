mod bubblewrap;
mod contree;
mod docker;
mod local;
pub mod output;
mod singularity;
mod swerex_docker;
mod swerex_modal;

pub use bubblewrap::BubblewrapEnvironment;
pub use contree::ContreeEnvironment;
pub use docker::DockerEnvironment;
pub use local::LocalEnvironment;
pub use output::{CommandOutput, check_finished};
pub use singularity::SingularityEnvironment;
pub use swerex_docker::SwerexDockerEnvironment;
pub use swerex_modal::SwerexModalEnvironment;

use crate::{Json, Map, Result, environments::output::kill_process_group, error::value_error};
use os_pipe::pipe;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

pub trait Environment: Send {
    fn execute(&mut self, action: &Json, cwd: &str, timeout: Option<u64>) -> Result<CommandOutput>;
    fn get_template_vars(&self) -> Map;
    fn serialize(&self) -> Json;
    fn type_name(&self) -> &str;
    fn config(&self) -> &Map;
    fn config_mut(&mut self) -> &mut Map;
    fn container_id(&self) -> Option<&str> {
        None
    }
    fn executable(&self) -> Option<&str> {
        None
    }
}

pub fn serialized_environment(type_name: &str, config: &Map) -> Json {
    json!({"info": {"config": {"environment": config, "environment_type": type_name}}})
}

pub fn system_template_vars() -> Map {
    json!({
        "system": match env::consts::OS { "macos" => "Darwin", "linux" => "Linux", "windows" => "Windows", other => other },
        "release": os_info_release(),
        "version": os_info_version(),
        "machine": env::consts::ARCH,
        "node": Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default(),
        "processor": env::consts::ARCH,
    }).as_object().cloned().unwrap_or_default()
}

fn os_info_release() -> String {
    #[cfg(target_os = "macos")]
    {
        Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().into())
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim()
            .to_owned()
    }
}

fn os_info_version() -> String {
    Command::new("uname")
        .arg("-v")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().into())
        .unwrap_or_default()
}

fn display_command(program: &str, args: &[OsString]) -> String {
    let mut parts = vec![program.to_owned()];
    for arg in args {
        parts.push(arg.to_string_lossy().into_owned());
    }
    parts.join(" ")
}

pub fn run_command(
    program: &str,
    args: &[OsString],
    cwd: &Path,
    env_vars: &HashMap<String, String>,
    timeout: u64,
    process_group: bool,
) -> CommandOutput {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .envs(env_vars)
        .stdin(Stdio::inherit());
    let (reader, writer) = match pipe() {
        Ok(pipes) => pipes,
        Err(error) => return CommandOutput::failure(String::new(), error, "OSError"),
    };
    command
        .stdout(Stdio::from(writer.try_clone().expect("pipe clone")))
        .stderr(Stdio::from(writer));
    #[cfg(unix)]
    if process_group {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return CommandOutput::failure(String::new(), error, "OSError"),
    };
    let pid = child.id();
    drop(command);
    let mut reader = reader;
    let reader_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map(|_| String::from_utf8_lossy(&bytes).into_owned())
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() >= Duration::from_secs(timeout) {
                    if process_group {
                        kill_process_group(pid);
                    } else {
                        let _ = child.kill();
                    }
                    let _ = child.wait();
                    break None;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                let _ = child.kill();
                let output = reader_thread
                    .join()
                    .unwrap_or(Ok(String::new()))
                    .unwrap_or_default();
                return CommandOutput::failure(output, error, "OSError");
            }
        }
    };
    let output = reader_thread
        .join()
        .unwrap_or(Ok(String::new()))
        .unwrap_or_default();
    match status {
        Some(status) => CommandOutput::success(output, status_code(status)),
        None => CommandOutput::failure(
            output,
            format!(
                "Command '{}' timed out after {} seconds",
                display_command(program, args),
                timeout
            ),
            "TimeoutExpired",
        ),
    }
}

#[cfg(unix)]
fn status_code(status: ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    status
        .signal()
        .map(|s| -(s as i64))
        .or_else(|| status.code().map(i64::from))
        .unwrap_or(-1)
}
#[cfg(windows)]
fn status_code(status: ExitStatus) -> i64 {
    status.code().map(i64::from).unwrap_or(-1)
}

pub fn string_list(config: &Map, key: &str) -> Vec<OsString> {
    config
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(OsString::from)
                .collect()
        })
        .unwrap_or_default()
}

pub fn string_map(config: &Map, key: &str) -> HashMap<String, String> {
    config
        .get(key)
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

pub fn timeout_value(config: &Map, override_timeout: Option<u64>) -> u64 {
    override_timeout
        .or_else(|| config.get("timeout").and_then(Value::as_u64))
        .unwrap_or(30)
}

pub fn action_command(action: &Json) -> String {
    action
        .get("command")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub fn environment_class_name(spec: &str) -> &str {
    spec.rsplit('.').next().unwrap_or(spec)
}

pub fn get_environment(mut config: Map, default_type: &str) -> Result<Box<dyn Environment>> {
    let spec = config
        .remove("environment_class")
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| default_type.to_owned());
    let name = environment_class_name(&spec);
    match name {
        "LocalEnvironment" | "local" => Ok(Box::new(LocalEnvironment::new(config))),
        "DockerEnvironment" | "docker" => Ok(Box::new(DockerEnvironment::new(config)?)),
        "SingularityEnvironment" | "singularity" => {
            Ok(Box::new(SingularityEnvironment::new(config)?))
        }
        "BubblewrapEnvironment" | "bubblewrap" => Ok(Box::new(BubblewrapEnvironment::new(config)?)),
        "SwerexDockerEnvironment" | "swerex_docker" => {
            Ok(Box::new(SwerexDockerEnvironment::new(config)?))
        }
        "ContreeEnvironment" | "contree" => Ok(Box::new(ContreeEnvironment::new(config)?)),
        "SwerexModalEnvironment" | "swerex_modal" => {
            Ok(Box::new(SwerexModalEnvironment::new(config)?))
        }
        _ => Err(value_error(format!("Unknown environment type: {spec}"))),
    }
}

#[allow(dead_code)]
fn _types(_: Option<PathBuf>, _: Option<Json>) {}
