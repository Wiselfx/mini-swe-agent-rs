use crate::{Json, content::get_content_string};

pub const OUTPUT_REMOVED: &str =
    "[output removed at compaction, run the command again if you need it]";

fn chars(text: &str) -> usize {
    text.chars().count()
}

pub fn estimate_tokens(messages: &[Json]) -> usize {
    messages
        .iter()
        .map(|m| chars(&get_content_string(m)) / 4 + 4)
        .sum()
}

pub fn context_tokens(messages: &[Json]) -> usize {
    for i in (0..messages.len()).rev() {
        if let Some(usage) = messages[i]
            .pointer("/extra/usage")
            .filter(|v| v.is_object())
        {
            let prompt = usage
                .get("prompt_tokens")
                .and_then(Json::as_u64)
                .unwrap_or(0) as usize;
            let completion = usage
                .get("completion_tokens")
                .and_then(Json::as_u64)
                .unwrap_or(0) as usize;
            return prompt + completion + estimate_tokens(&messages[i + 1..]);
        }
    }
    estimate_tokens(messages)
}

pub fn is_output(message: &Json) -> bool {
    message.get("role").and_then(Json::as_str) == Some("tool")
        || message.get("type").and_then(Json::as_str) == Some("function_call_output")
        || message.pointer("/extra/returncode").is_some()
}

pub fn is_human(message: &Json) -> bool {
    message.get("role").and_then(Json::as_str) == Some("user")
        && !is_output(message)
        && !matches!(
            message
                .pointer("/extra/interrupt_type")
                .and_then(Json::as_str),
            Some("FormatError" | "Compaction")
        )
}

pub fn strip_output(message: &Json) -> Json {
    let mut copy = message.clone();
    if let Some(extra) = copy.get_mut("extra").and_then(Json::as_object_mut) {
        extra.remove("usage");
        extra.remove("raw_output");
    }
    if is_output(&copy) {
        let key = if copy.get("type").and_then(Json::as_str) == Some("function_call_output") {
            "output"
        } else {
            "content"
        };
        if let Some(object) = copy.as_object_mut() {
            object.insert(key.into(), Json::String(OUTPUT_REMOVED.into()));
        }
    }
    copy
}

fn units(messages: &[Json]) -> Vec<Vec<usize>> {
    let mut result: Vec<Vec<usize>> = Vec::new();
    for (i, message) in messages.iter().enumerate() {
        if result.last().is_some() && is_output(message) && !is_human(message) {
            result.last_mut().unwrap().push(i);
        } else {
            result.push(vec![i]);
        }
    }
    result
}

pub struct Plan {
    pub first: Vec<usize>,
    pub gap1: Vec<usize>,
    pub middle: Vec<usize>,
    pub gap2: Vec<usize>,
    pub last: Vec<usize>,
    pub recent_start: usize,
}

pub fn plan(messages: &[Json], first: usize, middle: usize, last: usize, recent: f64) -> Plan {
    let all_units = units(messages);
    let units = &all_units[2..];
    let n = units.len();
    let mut kept = vec![0usize];
    let mut full = vec![0usize];
    for unit in units {
        kept.push(
            kept.last().unwrap()
                + estimate_tokens(
                    &unit
                        .iter()
                        .map(|&i| strip_output(&messages[i]))
                        .collect::<Vec<_>>(),
                ),
        );
        full.push(
            full.last().unwrap()
                + estimate_tokens(
                    &unit
                        .iter()
                        .map(|&i| messages[i].clone())
                        .collect::<Vec<_>>(),
                ),
        );
    }
    let n_first = (0..n).find(|&k| kept[k + 1] > first).unwrap_or(n);
    let n_last_raw = (1..n)
        .find(|&k| kept[n] - kept[n - k - 1] > last)
        .unwrap_or(n)
        .max(1);
    let n_last = n_last_raw.min(n.saturating_sub(n_first));
    let gap: Vec<usize> = (n_first..n - n_last).collect();
    let center = (kept[n_first] + kept[n - n_last]) as f64 / 2.0;
    let mid: Vec<usize> = gap
        .iter()
        .copied()
        .filter(|&k| ((kept[k] + kept[k + 1]) as f64 / 2.0 - center).abs() <= middle as f64 / 2.0)
        .collect();
    let recent_unit = (0..n)
        .find(|&k| full[k] as f64 >= (1.0 - recent) * full[n] as f64)
        .unwrap_or(n.saturating_sub(1));
    let pick = |ks: &[usize]| {
        ks.iter()
            .flat_map(|&k| units[k].clone())
            .collect::<Vec<_>>()
    };
    Plan {
        first: pick(&(0..n_first).collect::<Vec<_>>()),
        gap1: pick(
            &gap.iter()
                .copied()
                .filter(|&k| mid.is_empty() || k < mid[0])
                .collect::<Vec<_>>(),
        ),
        middle: pick(&mid),
        gap2: pick(
            &gap.iter()
                .copied()
                .filter(|&k| !mid.is_empty() && k > mid[mid.len() - 1])
                .collect::<Vec<_>>(),
        ),
        last: pick(&(n - n_last..n).collect::<Vec<_>>()),
        recent_start: units[recent_unit][0],
    }
}
