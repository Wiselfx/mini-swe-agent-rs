use clap::Parser;
use mini_swe_agent::{
    Json, Map, Result,
    agents::get_agent,
    config::{get_config_from_spec, recursive_merge},
    content::get_content_string,
    environments::get_environment,
    models::get_model,
    run::{
        init,
        utilities::{
            config::{configure_if_first_time, prompt},
            sessions::new_session_path,
        },
    },
};
use serde_json::{Value, json};
use std::{collections::HashMap, io::IsTerminal, path::PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "mini",
    about = "Run mini-SWE-agent in your local environment.",
    version
)]
struct Args {
    #[arg(short = 'm', long)]
    model: Option<String>,
    #[arg(long)]
    model_class: Option<String>,
    #[arg(long)]
    agent_class: Option<String>,
    #[arg(long)]
    environment_class: Option<String>,
    #[arg(short = 't', long)]
    task: Option<String>,
    #[arg(short = 'y', long)]
    yolo: bool,
    #[arg(short = 'l', long)]
    cost_limit: Option<f64>,
    #[arg(short = 'c', long = "config", default_values_t = [default_config()])]
    config: Vec<String>,
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    #[arg(long)]
    exit_immediately: bool,
    #[arg(short = 'r', long)]
    resume: bool,
}

fn default_config() -> String {
    std::env::var("MSWEA_MINI_CONFIG_PATH").unwrap_or_else(|_| "mini.yaml".into())
}

fn main() {
    init();
    if let Err(error) = execute(Args::parse()) {
        mini_swe_agent::run::report_error(&error);
        std::process::exit(1);
    }
}

fn execute(args: Args) -> Result<()> {
    configure_if_first_time()?;
    println!("Building agent config from specs: {:?}", args.config);
    let mut configs = args
        .config
        .iter()
        .map(|s| get_config_from_spec(s))
        .collect::<Result<Vec<_>>>()?;
    let mut saved = None;
    let mut output = args.output;
    if args.resume {
        if output.is_none() && !std::io::stdin().is_terminal() {
            return Err(mini_swe_agent::error::value_error(
                "Without a terminal, pass the session file to resume with -o.",
            ));
        }
        let path = match output.clone() {
            Some(path) => path,
            None => mini_swe_agent::run::utilities::sessions::pick_session()?,
        };
        let data: Json = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        let model = data
            .pointer("/info/config/model")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let model_type = data
            .pointer("/info/config/model_type")
            .cloned()
            .unwrap_or(Value::Null);
        let mut model = model.as_object().cloned().unwrap_or_default();
        if !model_type.is_null() {
            model.insert("model_class".into(), model_type);
        }
        configs.push(json!({"model":model}));
        println!("Resuming {}", path.display());
        saved = Some(data);
        output = Some(path);
    }
    let mut overrides = Map::new();
    if args.task.is_some() {
        overrides.insert("task".into(), json!(args.task));
    }
    let mut agent = Map::new();
    if args.agent_class.is_some() {
        agent.insert("agent_class".into(), json!(args.agent_class));
    }
    if args.yolo {
        agent.insert("mode".into(), json!("yolo"));
    }
    if let Some(limit) = args.cost_limit {
        agent.insert("cost_limit".into(), json!(limit));
    }
    if args.exit_immediately || !std::io::stdin().is_terminal() {
        agent.insert("confirm_exit".into(), json!(false));
    }
    if let Some(path) = &output {
        agent.insert("output_path".into(), json!(path.to_string_lossy()));
    }
    let mut model = Map::new();
    if args.model_class.is_some() {
        model.insert("model_class".into(), json!(args.model_class));
    }
    if args.model.is_some() {
        model.insert("model_name".into(), json!(args.model));
    }
    let mut environment = Map::new();
    if args.environment_class.is_some() {
        environment.insert("environment_class".into(), json!(args.environment_class));
    }
    configs.push(json!({"run":overrides,"agent":agent,"model":model,"environment":environment}));
    let config = recursive_merge(configs);
    let model = get_model(
        config
            .get("model")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
    )?;
    let env = get_environment(
        config
            .get("environment")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        "local",
    )?;
    let mut agent = get_agent(
        model,
        env,
        config
            .get("agent")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        "interactive",
    )?;
    let mut messages = None;
    if let Some(saved) = saved {
        agent.cost = saved
            .pointer("/info/model_stats/instance_cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        agent.n_calls = saved
            .pointer("/info/model_stats/api_calls")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        agent.compacted_history = saved
            .get("compacted_history")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        messages = saved.get("messages").and_then(Value::as_array).cloned();
        if let Some(last) = messages.as_ref().and_then(|m| {
            m.iter().rev().find(|m| {
                m.get("role").and_then(Value::as_str) == Some("assistant")
                    && !get_content_string(m).trim().is_empty()
            })
        }) {
            let text = get_content_string(last);
            println!(
                "Last reply:\n{}",
                text.chars()
                    .rev()
                    .take(2000)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>()
            );
        }
    }
    let task = match config
        .get("run")
        .and_then(|r| r.get("task"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(task) => task.to_owned(),
        None => {
            if agent.interactive && std::io::stdin().is_terminal() && messages.is_none() {
                agent.preflight()?;
            }
            println!("What do you want to do?");
            multiline_prompt()?
        }
    };
    if agent.output_path().is_none() {
        agent.set_output_path(Some(new_session_path(&task)));
    }
    agent.run(task, messages, Map::new())?;
    if let Some(path) = agent.output_path() {
        println!("Saved trajectory to '{}'", path.display());
    }
    Ok(())
}

fn multiline_prompt() -> Result<String> {
    println!("Enter your task. Finish with a single '.' on its own line.");
    let mut lines = Vec::new();
    loop {
        let line = prompt("", "")?;
        if line == "." {
            break;
        }
        lines.push(line);
    }
    Ok(lines.join("\n"))
}

#[allow(dead_code)]
fn _types(_: Option<HashMap<String, String>>) {}
