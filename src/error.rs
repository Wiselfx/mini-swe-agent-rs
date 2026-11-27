use crate::{Json, Result};
use serde_json::json;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Format,
    Submitted,
    WaitForUser,
    Limits,
    Time,
    UserInterruption,
    UserReply,
    UserNewTask,
    UserRejection,
    Other,
}

#[derive(Debug)]
pub struct MiniError {
    pub kind: ErrorKind,
    pub messages: Vec<Json>,
    pub source: Option<anyhow::Error>,
}

impl MiniError {
    pub fn new(kind: ErrorKind, message: Json) -> Self {
        Self {
            kind,
            messages: vec![message],
            source: None,
        }
    }

    pub fn other(message: impl fmt::Display) -> Self {
        Self {
            kind: ErrorKind::Other,
            messages: vec![],
            source: Some(anyhow::anyhow!(message.to_string())),
        }
    }

    pub fn from_source(source: anyhow::Error) -> Self {
        Self {
            kind: ErrorKind::Other,
            messages: vec![],
            source: Some(source),
        }
    }

    pub fn exit(status: &str) -> Self {
        let kind = match status {
            "LimitsExceeded" => ErrorKind::Limits,
            "TimeExceeded" => ErrorKind::Time,
            _ => ErrorKind::Other,
        };
        Self::new(
            kind,
            json!({
                "role": "exit", "content": status,
                "extra": {"exit_status": status, "submission": ""}
            }),
        )
    }

    pub fn submitted(submission: String) -> Self {
        Self::new(
            ErrorKind::Submitted,
            json!({
                "role": "exit", "content": submission,
                "extra": {"exit_status": "Submitted", "submission": submission}
            }),
        )
    }

    pub fn type_name(&self) -> &'static str {
        match self.kind {
            ErrorKind::Format => "FormatError",
            ErrorKind::Submitted => "Submitted",
            ErrorKind::WaitForUser => "WaitForUser",
            ErrorKind::Limits => "LimitsExceeded",
            ErrorKind::Time => "TimeExceeded",
            ErrorKind::UserInterruption => "UserInterruption",
            ErrorKind::UserReply => "UserReply",
            ErrorKind::UserNewTask => "UserNewTask",
            ErrorKind::UserRejection => "UserRejection",
            ErrorKind::Other => "Error",
        }
    }
}

impl fmt::Display for MiniError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(source) = &self.source {
            source.fmt(f)
        } else {
            write!(f, "{}", self.type_name())
        }
    }
}

impl std::error::Error for MiniError {}

impl From<anyhow::Error> for MiniError {
    fn from(value: anyhow::Error) -> Self {
        Self::from_source(value)
    }
}
impl From<std::io::Error> for MiniError {
    fn from(value: std::io::Error) -> Self {
        Self::from_source(value.into())
    }
}
impl From<serde_json::Error> for MiniError {
    fn from(value: serde_json::Error) -> Self {
        Self::from_source(value.into())
    }
}
impl From<serde_yaml::Error> for MiniError {
    fn from(value: serde_yaml::Error) -> Self {
        Self::from_source(value.into())
    }
}
impl From<reqwest::Error> for MiniError {
    fn from(value: reqwest::Error) -> Self {
        Self::from_source(value.into())
    }
}
impl From<minijinja::Error> for MiniError {
    fn from(value: minijinja::Error) -> Self {
        Self::from_source(value.into())
    }
}
impl From<regex::Error> for MiniError {
    fn from(value: regex::Error) -> Self {
        Self::from_source(value.into())
    }
}

pub fn context(message: &str) -> MiniError {
    MiniError::other(message)
}
pub fn value_error(message: impl fmt::Display) -> MiniError {
    MiniError::other(message)
}
pub fn ensure(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(value_error(message))
    }
}
