use clap::Parser;
use mini_swe_agent::{
    Map, Result,
    agents::get_agent,
    models::get_model,
    run::{
        benchmarks::swebench::{build_config, get_sb_environment, load_dataset},
        init,
    },
};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "swebench-single", version)]
struct Args {
    #[arg(long, default_value = "lite")]
    subset: String,
    #[arg(long, default_value = "dev")]
    split: String,
    #[arg(short = 'i', long, default_value = "0")]
    instance: String,
    #[arg(short = 'm', long)]
    model: Option<String>,
    #[arg(long)]
    model_class: Option<String>,
    #[arg(long)]
    agent_class: Option<String>,
    #[arg(long)]
    environment_class: Option<String>,
    #[arg(short = 'y', long)]
    yolo: bool,
    #[arg(short = 'l', long)]
    cost_limit: Option<f64>,
    #[arg(short = 'c', long = "config", default_values = ["swebench.yaml"])]
    config: Vec<String>,
    #[arg(long)]
    exit_immediately: bool,
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
}

fn main() {
    init();
    if let Err(error) = execute(Args::parse()) {
        mini_swe_agent::run::report_error(&error);
        std::process::exit(1);
    }
}

fn execute(args: Args) -> Result<()> {
    let instances = load_dataset(&args.subset, &args.split)?;
    let mut by_id = std::collections::HashMap::new();
    for instance in &instances {
        if let Some(id) = instance.get("instance_id").and_then(Value::as_str) {
            by_id.insert(id.to_owned(), instance);
        }
    }
    let mut ids: Vec<_> = by_id.keys().cloned().collect();
    ids.sort();
    let id = if args.instance.chars().all(|c| c.is_ascii_digit()) {
        ids.get(
            args.instance
                .parse::<usize>()
                .map_err(mini_swe_agent::error::value_error)?,
        )
        .cloned()
        .ok_or_else(|| mini_swe_agent::error::value_error("Instance index out of range"))?
    } else {
        args.instance.clone()
    };
    let instance = by_id
        .get(&id)
        .cloned()
        .ok_or_else(|| mini_swe_agent::error::value_error(format!("Unknown instance {id}")))?;
    let mut config = build_config(
        &args.config,
        args.model.as_deref(),
        args.model_class.as_deref(),
        args.environment_class.as_deref(),
    )?;
    let mut agent = Map::new();
    if let Some(class) = args.agent_class {
        agent.insert("agent_class".into(), json!(class));
    }
    if args.yolo {
        agent.insert("mode".into(), json!("yolo"));
    }
    if let Some(limit) = args.cost_limit {
        agent.insert("cost_limit".into(), json!(limit));
    }
    if args.exit_immediately {
        agent.insert("confirm_exit".into(), json!(false));
    }
    let output = args.output.unwrap_or_else(|| {
        mini_swe_agent::run::global_config_dir().join("last_swebench_single_run.traj.json")
    });
    agent.insert("output_path".into(), json!(output.to_string_lossy()));
    config
        .as_object_mut()
        .unwrap()
        .insert("agent".into(), Value::Object(agent));
    let env = get_sb_environment(&config, instance)?;
    let model = get_model(
        config
            .get("model")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
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
    agent.run(
        instance
            .get("problem_statement")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        None,
        Map::new(),
    )?;
    Ok(())
}
