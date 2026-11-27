use super::config;
use crate::Result;
use crate::run::global_config_dir;
use std::path::PathBuf;

pub fn cwd_dir() -> PathBuf {
    let slug = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    global_config_dir().join("sessions").join(slug)
}

pub fn list_sessions() -> Vec<PathBuf> {
    let root = global_config_dir().join("sessions");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(|e| e.ok())
        .flat_map(|e| {
            std::fs::read_dir(e.path())
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
        })
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("json")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".traj.json"))
        })
        .collect::<Vec<_>>();
    paths.sort_by(|a, b| {
        b.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
            .cmp(
                &a.metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH),
            )
    });
    paths
}

pub fn pick_session() -> Result<PathBuf> {
    let sessions = list_sessions();
    if sessions.is_empty() {
        return Err(crate::error::value_error(format!(
            "No sessions found in {}",
            global_config_dir().join("sessions").display()
        )));
    }
    for (i, path) in sessions.iter().take(20).enumerate() {
        println!("{}. {}", i + 1, path.display());
    }
    let answer = config::prompt("resume> ", "")?;
    if answer.is_empty() {
        return Ok(sessions[0].clone());
    }
    if let Ok(index) = answer.parse::<usize>() {
        if index > 0 && index <= sessions.len() {
            return Ok(sessions[index - 1].clone());
        }
    }
    sessions
        .into_iter()
        .find(|p| p.to_string_lossy().contains(&answer))
        .ok_or_else(|| crate::error::value_error(format!("No session matches {answer:?}")))
}

pub fn new_session_path(task: &str) -> PathBuf {
    let first = task.trim().lines().next().unwrap_or("").to_lowercase();
    let slug: String = first
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(40)
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    let now = chrono::Local::now().format("%Y%m%d-%H%M%S");
    cwd_dir().join(format!(
        "{now}-{}.traj.json",
        if slug.is_empty() { "session" } else { &slug }
    ))
}
