use crate::Map;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

pub struct ProgressManager {
    pub total: usize,
    pub completed: usize,
    pub statuses: HashMap<String, Vec<String>>,
    pub current: HashMap<String, String>,
    pub report: Option<PathBuf>,
    pub started: Instant,
}

impl ProgressManager {
    pub fn new(total: usize, report: PathBuf) -> Self {
        Self {
            total,
            completed: 0,
            statuses: HashMap::new(),
            current: HashMap::new(),
            report: Some(report),
            started: Instant::now(),
        }
    }
    pub fn start(&mut self, id: &str) {
        self.current
            .insert(id.to_owned(), "Task initialized".into());
        self.render();
    }
    pub fn status(&mut self, id: &str, status: &str) {
        self.current.insert(id.to_owned(), status.to_owned());
    }
    pub fn end(&mut self, id: &str, status: &str) {
        self.current.remove(id);
        self.statuses
            .entry(status.to_owned())
            .or_default()
            .push(id.to_owned());
        self.completed += 1;
        self.save();
        self.render();
    }
    pub fn render(&self) {
        let cost = crate::models::stats::global_cost();
        println!(
            "\rOverall {}/{} (${:.2})  {}",
            self.completed,
            self.total,
            cost,
            self.statuses
                .iter()
                .map(|(s, v)| format!("{s}:{}", v.len()))
                .collect::<Vec<_>>()
                .join("  ")
        );
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    pub fn save(&self) {
        if let Some(path) = &self.report {
            let _ = std::fs::write(
                path,
                serde_yaml::to_string(&json_statuses(&self.statuses)).unwrap_or_default(),
            );
        }
    }
    pub fn print_report(&self) {
        println!();
        for (status, ids) in &self.statuses {
            println!("{status}: {}", ids.len());
            for id in ids {
                println!("  {id}");
            }
        }
    }
}

fn json_statuses(statuses: &HashMap<String, Vec<String>>) -> serde_json::Value {
    serde_json::json!({"instances_by_exit_status":statuses})
}

pub fn filter_instances(
    mut instances: Vec<serde_json::Value>,
    filter_spec: &str,
    slice_spec: &str,
    shuffle: bool,
) -> Vec<serde_json::Value> {
    if shuffle {
        instances.sort_by_key(|i| {
            i.get("instance_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        });
        let mut seed = 42u64;
        for i in (1..instances.len()).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let j = (seed as usize) % (i + 1);
            instances.swap(i, j);
        }
    }
    if !filter_spec.is_empty() {
        let re =
            regex::Regex::new(filter_spec).unwrap_or_else(|_| regex::Regex::new("$^").unwrap());
        instances
            .retain(|i| re.is_match(i.get("instance_id").and_then(Value::as_str).unwrap_or("")));
    }
    if !slice_spec.is_empty() {
        let values: Vec<Option<usize>> = slice_spec
            .split(':')
            .map(|s| if s.is_empty() { None } else { s.parse().ok() })
            .collect();
        let start = values.first().copied().flatten().unwrap_or(0);
        let end = values.get(1).copied().flatten();
        let step = values.get(2).copied().flatten();
        instances = instances
            .into_iter()
            .skip(start)
            .take(end.map(|e| e.saturating_sub(start)).unwrap_or(usize::MAX))
            .enumerate()
            .filter(|(i, _)| step.is_none_or(|s| s == 0 || i % s == 0))
            .map(|(_, v)| v)
            .collect();
    }
    instances
}

#[allow(dead_code)]
fn _types(_: Option<Map>, _: Option<Arc<Mutex<()>>>, _: Option<&Path>) {}
