use crate::{Map, Result};
use serde_json::json;
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct Stats {
    cost: f64,
    calls: u64,
}

static STATS: OnceLock<Mutex<Stats>> = OnceLock::new();
static LIMITS: OnceLock<(f64, u64)> = OnceLock::new();

fn stats() -> &'static Mutex<Stats> {
    STATS.get_or_init(|| Mutex::new(Stats::default()))
}

pub fn global_cost() -> f64 {
    stats().lock().unwrap().cost
}
pub fn global_calls() -> u64 {
    stats().lock().unwrap().calls
}

pub fn add(cost: f64) -> Result<()> {
    let (cost_limit, call_limit) = *LIMITS.get_or_init(|| {
        (
            std::env::var("MSWEA_GLOBAL_COST_LIMIT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            std::env::var("MSWEA_GLOBAL_CALL_LIMIT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        )
    });
    let mut stats = stats().lock().unwrap();
    stats.cost += cost;
    stats.calls += 1;
    if 0.0 < cost_limit && cost_limit < stats.cost || 0 < call_limit && call_limit < stats.calls + 1
    {
        return Err(crate::error::value_error(format!(
            "Global cost/call limit exceeded: ${:.4} / {}",
            stats.cost, stats.calls
        )));
    }
    Ok(())
}

pub fn _types(_: Option<Map>) {
    let _ = json!({});
}
