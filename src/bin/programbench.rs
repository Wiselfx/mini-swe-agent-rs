use clap::Parser;
use mini_swe_agent::{
    Json, Map, Result,
    agents::DefaultAgent,
    environments::get_environment,
    error::value_error,
    models::get_model,
    run::{
        benchmarks::{
            progress::{ProgressManager, filter_instances},
            swebench::{build_config, load_local_dataset},
        },
        init,
    },
};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Parser)]
#[command(name = "programbench", version)]
struct Args {
    #[arg(long, default_value = "")]
    slice: String,
    #[arg(long, default_value = "")]
    filter: String,
    #[arg(long)]
    shuffle: bool,
    #[arg(short = 'o', long, default_value = "")]
    output: String,
    #[arg(short = 'w', long, default_value_t = 1)]
    workers: usize,
    #[arg(short = 'm', long)]
    model: Option<String>,
    #[arg(long)]
    model_class: Option<String>,
    #[arg(long)]
    redo_existing: bool,
    #[arg(short = 'c', long = "config", default_values = ["programbench.yaml"])]
    config: Vec<String>,
    #[arg(long)]
    environment_class: Option<String>,
}

fn main() {
    init();
    if let Err(error) = execute(Args::parse()) {
        mini_swe_agent::run::report_error(&error);
        std::process::exit(1);
    }
}

fn execute(args: Args) -> Result<()> {
    let dataset = env::var("PROGRAMBENCH_DATASET").map_err(|_| {
        value_error("Set PROGRAMBENCH_DATASET to a local ProgramBench JSON/JSONL dataset")
    })?;
    let instances = load_local_dataset(Path::new(&dataset))?;
    let instances = filter_instances(instances, &args.filter, &args.slice, args.shuffle);
    let output = PathBuf::from(if args.output.is_empty() {
        format!(
            "programbench_results_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        )
    } else {
        args.output.clone()
    });
    fs::create_dir_all(&output)?;
    if !args.redo_existing { /* completed submissions are skipped below */ }
    let config = Arc::new(build_config(
        &args.config,
        args.model.as_deref(),
        args.model_class.as_deref(),
        args.environment_class.as_deref(),
    )?);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let progress = Arc::new(Mutex::new(ProgressManager::new(
        instances.len(),
        output.join(format!("exit_statuses_{stamp}.yaml")),
    )));
    let output = Arc::new(output);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(args.workers.max(1))
        .build()
        .map_err(mini_swe_agent::error::value_error)?;
    pool.install(|| {
        rayon::scope(|scope| {
            for instance in instances {
                let id = instance
                    .get("instance_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if !args.redo_existing && output.join(&id).join("submission.tar.gz").exists() {
                    continue;
                }
                let config = config.clone();
                let output = output.clone();
                let progress = progress.clone();
                scope.spawn(move |_| {
                    if let Err(error) = process(instance, &output, &config, &progress) {
                        mini_swe_agent::run::logging::error(error);
                    }
                });
            }
        });
    });
    progress.lock().unwrap().print_report();
    Ok(())
}

fn process(
    instance: Json,
    output: &Path,
    config: &Json,
    progress: &Arc<Mutex<ProgressManager>>,
) -> Result<()> {
    let id = instance
        .get("instance_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let dir = output.join(&id);
    fs::create_dir_all(&dir)?;
    let _ = fs::remove_file(dir.join(format!("{id}.traj.json")));
    progress.lock().unwrap().start(&id);
    let mut inst_config = config.clone();
    let image = format!(
        "{}:task_cleanroom_v6",
        instance
            .get("image_name")
            .and_then(Value::as_str)
            .unwrap_or("")
    );
    inst_config
        .as_object_mut()
        .unwrap()
        .entry("environment".to_owned())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap()
        .insert("image".into(), json!(image));
    let result = (|| -> Result<()> {
        let mut env = get_environment(
            inst_config
                .get("environment")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            "docker",
        )?;
        env.execute(&json!({"command":"git config user.name \"mini-swe-agent\" && git config user.email \"mini-swe-agent@proton.me\""}), "", None)?;
        let mut agent_config = inst_config
            .get("agent")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        agent_config.insert(
            "output_path".into(),
            json!(dir.join(format!("{id}.traj.json")).to_string_lossy()),
        );
        let model = get_model(
            inst_config
                .get("model")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        )?;
        let mut agent = DefaultAgent::new(model, env, agent_config, false);
        let mut extra = Map::new();
        extra.insert("instance".into(), instance.clone());
        let info = agent.run(String::new(), None, extra)?;
        copy_submission(agent.env.as_mut(), &dir.join("submission.tar.gz"))?;
        let mut data = agent.serialize();
        strip_raw_output(&mut data);
        data.as_object_mut()
            .unwrap()
            .insert("instance_id".into(), json!(id));
        let exit_status = info
            .get("exit_status")
            .cloned()
            .unwrap_or_else(|| json!(""));
        if let Some(info_object) = data.pointer_mut("/info").and_then(Value::as_object_mut) {
            info_object.insert("exit_status".into(), exit_status);
        }
        fs::write(
            dir.join(format!("{id}.traj.json")),
            serde_json::to_string_pretty(&data)?,
        )?;
        let status = info
            .get("exit_status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        progress.lock().unwrap().end(&id, &status);
        Ok(())
    })();
    if let Err(error) = result {
        progress.lock().unwrap().end(&id, error.type_name());
        return Err(error);
    }
    Ok(())
}

fn strip_raw_output(value: &mut Json) {
    match value {
        Value::Array(items) => {
            for item in items {
                strip_raw_output(item)
            }
        }
        Value::Object(object) => {
            object.remove("raw_output");
            for child in object.values_mut() {
                strip_raw_output(child)
            }
        }
        _ => {}
    }
}

fn copy_submission(
    env: &mut dyn mini_swe_agent::environments::Environment,
    dest: &Path,
) -> Result<()> {
    let id = env
        .container_id()
        .ok_or_else(|| value_error("copy_submission requires Docker"))?
        .to_owned();
    let executable = env
        .executable()
        .ok_or_else(|| value_error("copy_submission requires Docker"))?
        .to_owned();
    let output = env.execute(
        &json!({"command":"tar -czf /tmp/_submission.tar.gz -C /workspace ."}),
        "",
        None,
    )?;
    if output.returncode != 0 {
        return Err(value_error(output.output));
    }
    let status = Command::new(&executable)
        .args(["cp"])
        .arg(format!("{id}:/tmp/_submission.tar.gz"))
        .arg(dest)
        .status()?;
    if !status.success() {
        return Err(value_error("docker cp failed"));
    }
    Ok(())
}
