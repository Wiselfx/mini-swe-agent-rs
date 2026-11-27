use clap::Parser;
use mini_swe_agent::{
    Map, Result, agents::DefaultAgent, config::builtin_config, environments::LocalEnvironment,
    models::get_model, run::init,
};
use serde_json::Value;

#[derive(Parser)]
#[command(name = "hello-world", version)]
struct Args {
    #[arg(short = 't', long)]
    task: String,
    #[arg(short = 'm', long)]
    model: Option<String>,
}

fn main() {
    init();
    if let Err(error) = execute(Args::parse()) {
        mini_swe_agent::run::report_error(&error);
        std::process::exit(1);
    }
}

fn execute(args: Args) -> Result<()> {
    let model_name = mini_swe_agent::models::get_model_name(args.model.as_deref(), &Map::new())?;
    let config: Value = serde_yaml::from_str(&builtin_config("default.yaml")?)?;
    let agent_config = config
        .get("agent")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let model = get_model(Map::from_iter([(
        "model_name".into(),
        Value::String(model_name),
    )]))?;
    let mut agent = DefaultAgent::new(
        model,
        Box::new(LocalEnvironment::new(Map::new())),
        agent_config,
        false,
    );
    agent.run(args.task, None, Map::new())?;
    Ok(())
}
