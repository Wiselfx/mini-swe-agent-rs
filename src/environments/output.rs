use crate::{Json, Map, Result};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct CommandOutput {
    pub output: String,
    pub returncode: i64,
    pub exception_info: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map>,
}

impl CommandOutput {
    pub fn success(output: String, returncode: i64) -> Self {
        Self {
            output,
            returncode,
            exception_info: String::new(),
            extra: None,
        }
    }

    pub fn failure(output: String, error: impl std::fmt::Display, exception_type: &str) -> Self {
        let text = error.to_string();
        Self {
            output,
            returncode: -1,
            exception_info: format!("An error occurred while executing the command: {text}"),
            extra: Some(Map::from_iter([
                ("exception_type".into(), json!(exception_type)),
                ("exception".into(), json!(text)),
            ])),
        }
    }

    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

pub fn check_finished(output: &CommandOutput) -> Result<()> {
    if output.returncode != 0 {
        return Ok(());
    }
    let trimmed = output.output.trim_start();
    let Some((first, rest)) = trimmed.split_once('\n') else {
        if trimmed.trim() == "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT" {
            return Err(crate::error::MiniError::submitted(String::new()));
        }
        return Ok(());
    };
    if first.trim() == "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT" {
        return Err(crate::error::MiniError::submitted(rest.to_owned()));
    }
    Ok(())
}

#[cfg(unix)]
pub fn kill_process_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(windows)]
pub fn kill_process_group(_pid: u32) {}
