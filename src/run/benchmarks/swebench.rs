use crate::{
    Json, Map, Result,
    agents::DefaultAgent,
    config::recursive_merge,
    environments::{Environment, get_environment},
    error::value_error,
    models::get_model,
    run::{benchmarks::progress::ProgressManager, logging},
    template::render,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub const DATASET_MAPPING: [(&str, &str); 8] = [
    ("full", "princeton-nlp/SWE-Bench"),
    ("verified", "princeton-nlp/SWE-Bench_Verified"),
    ("lite", "princeton-nlp/SWE-Bench_Lite"),
    ("multimodal", "princeton-nlp/SWE-Bench_Multimodal"),
    ("multilingual", "swe-bench/SWE-Bench_Multilingual"),
    ("smith", "SWE-bench/SWE-smith"),
    ("_test", "klieret/swe-bench-dummy-test-dataset"),
    ("rebench", "nebius/SWE-rebench"),
];

pub fn dataset_name(subset: &str) -> &str {
    DATASET_MAPPING
        .iter()
        .find(|(name, _)| *name == subset)
        .map(|(_, path)| *path)
        .unwrap_or(subset)
}

pub fn load_dataset(subset: &str, split: &str) -> Result<Vec<Json>> {
    let name = dataset_name(subset);
    let path = Path::new(name);
    if path.exists() {
        return load_local_dataset(path);
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let splits_url = format!(
        "https://datasets-server.huggingface.co/splits?{}",
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&[("dataset", name)])
            .finish()
    );
    let splits: Json = client.get(splits_url).send()?.error_for_status()?.json()?;
    let config = splits
        .pointer("/splits")
        .and_then(Value::as_array)
        .and_then(|all| {
            all.iter()
                .find(|s| s.get("split").and_then(Value::as_str) == Some(split))
        })
        .and_then(|s| s.get("config"))
        .and_then(Value::as_str)
        .unwrap_or("default");
    let mut rows = Vec::new();
    let mut offset = 0usize;
    loop {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&[
                ("dataset", name),
                ("config", config),
                ("split", split),
                ("offset", &offset.to_string()),
                ("length", "100"),
            ])
            .finish();
        let response: Json = client
            .get(format!(
                "https://datasets-server.huggingface.co/rows?{query}"
            ))
            .send()?
            .error_for_status()?
            .json()?;
        let batch = response
            .pointer("/rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if batch.is_empty() {
            break;
        }
        offset += batch.len();
        rows.extend(batch.into_iter().filter_map(|r| r.get("row").cloned()));
        if response
            .get("num_rows_total")
            .and_then(Value::as_u64)
            .is_some_and(|n| offset as u64 >= n)
        {
            break;
        }
    }
    Ok(rows)
}

pub fn load_local_dataset(path: &Path) -> Result<Vec<Json>> {
    let text = fs::read_to_string(path)?;
    if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
        return text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| Ok(serde_json::from_str(l)?))
            .collect();
    }
    let value: Json = serde_json::from_str(&text)?;
    if let Some(rows) = value.as_array() {
        Ok(rows.clone())
    } else if let Some(rows) = value
        .get("data")
        .or_else(|| value.get("rows"))
        .and_then(Value::as_array)
    {
        Ok(rows.clone())
    } else {
        Err(value_error("Unrecognized dataset format"))
    }
}

pub fn get_swebench_docker_image_name(instance: &Json) -> String {
    if let Some(image) = instance
        .get("image_name")
        .or_else(|| instance.get("docker_image"))
        .and_then(Value::as_str)
    {
        return image.to_owned();
    }
    let id = instance
        .get("instance_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .replace("__", "_1776_");
    format!("docker.io/swebench/sweb.eval.x86_64.{id}:latest").to_lowercase()
}

pub fn get_sb_environment(config: &Json, instance: &Json) -> Result<Box<dyn Environment>> {
    let mut env_config = config
        .get("environment")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    env_config
        .entry("environment_class".to_owned())
        .or_insert_with(|| json!("docker"));
    let class = env_config
        .get("environment_class")
        .and_then(Value::as_str)
        .unwrap_or("");
    let image = get_swebench_docker_image_name(instance);
    if matches!(class, "docker" | "swerex_modal" | "DockerEnvironment") {
        env_config.insert("image".into(), json!(image));
    } else if matches!(class, "singularity" | "contree" | "SingularityEnvironment") {
        env_config.insert("image".into(), json!(format!("docker://{image}")));
    }
    let mut env = get_environment(env_config, "docker")?;
    if let Some(command) = config
        .pointer("/run/env_startup_command")
        .and_then(Value::as_str)
    {
        let vars = instance.as_object().cloned().unwrap_or_default();
        let command = render(command, &vars)?;
        let output = env.execute(&json!({"command":command}), "", None)?;
        if output.returncode != 0 {
            return Err(value_error(format!(
                "Error executing startup command: {}",
                output.to_json()
            )));
        }
    }
    Ok(env)
}

static PREDS_LOCK: Mutex<()> = Mutex::new(());

pub fn update_preds_file(
    path: &Path,
    instance_id: &str,
    model_name: &str,
    result: Option<&str>,
) -> Result<()> {
    let _guard = PREDS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut data = if path.exists() {
        serde_json::from_str::<Json>(&fs::read_to_string(path)?)?
    } else {
        json!({})
    };
    let object = data
        .as_object_mut()
        .ok_or_else(|| value_error("preds.json must contain an object"))?;
    if let Some(result) = result {
        object.insert(
            instance_id.into(),
            json!({"model_name_or_path":model_name,"instance_id":instance_id,"model_patch":result}),
        );
    } else {
        object.remove(instance_id);
    }
    fs::write(path, serde_json::to_string_pretty(&data)?)?;
    Ok(())
}

pub fn process_instance(
    instance: Json,
    output_dir: &Path,
    config: &Json,
    progress: &Arc<Mutex<ProgressManager>>,
) -> Result<()> {
    let id = instance
        .get("instance_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let instance_dir = output_dir.join(&id);
    update_preds_file(&output_dir.join("preds.json"), &id, "", None)?;
    let _ = fs::remove_file(instance_dir.join(format!("{id}.traj.json")));
    let model_config = config
        .get("model")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let model = get_model(model_config)?;
    let model_name = model
        .config()
        .get("model_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    progress.lock().unwrap().start(&id);
    let task = instance
        .get("problem_statement")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut agent: Option<DefaultAgent> = None;
    let status: String;
    let submission: String;
    let mut extra = Map::new();
    let result = (|| -> Result<Json> {
        let env = get_sb_environment(config, &instance)?;
        let agent_config = config
            .get("agent")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        agent = Some(DefaultAgent::new(model, env, agent_config, false));
        agent.as_mut().unwrap().run(task, None, Map::new())
    })();
    match result {
        Ok(info) => {
            status = info
                .get("exit_status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into();
            submission = info
                .get("submission")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into();
        }
        Err(error) => {
            status = error.type_name().into();
            submission = String::new();
            extra.insert("traceback".into(), json!(error.to_string()));
            extra.insert("exception_str".into(), json!(error.to_string()));
        }
    }
    if let Some(agent) = &agent {
        let path = instance_dir.join(format!("{id}.traj.json"));
        let mut extra_data =
            json!({"info":{"exit_status":status,"submission":submission},"instance_id":id});
        for (key, value) in extra {
            extra_data
                .pointer_mut("/info")
                .and_then(Value::as_object_mut)
                .unwrap()
                .insert(key, value);
        }
        agent.save(Some(path), vec![extra_data])?;
    }
    update_preds_file(
        &output_dir.join("preds.json"),
        &id,
        &model_name,
        Some(&submission),
    )?;
    progress.lock().unwrap().end(&id, &status);
    Ok(())
}

pub fn run_batch(
    instances: Vec<Json>,
    output_dir: &Path,
    config: Json,
    workers: usize,
    redo: bool,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    logging::add_file_handler(&output_dir.join("minisweagent.log"))?;
    let mut instances = instances;
    if !redo {
        let existing: HashMap<_, _> = if output_dir.join("preds.json").exists() {
            let data: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(output_dir.join("preds.json"))?)?;
            data.as_object()
                .map(|m| m.keys().map(|k| (k.clone(), ())).collect())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        instances.retain(|i| {
            !existing.contains_key(i.get("instance_id").and_then(Value::as_str).unwrap_or(""))
        });
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let report = output_dir.join(format!("exit_statuses_{stamp}.yaml"));
    let progress = Arc::new(Mutex::new(ProgressManager::new(instances.len(), report)));
    logging::info(format!("Running on {} instances...", instances.len()));
    let config = Arc::new(config);
    let output_dir = Arc::new(output_dir.to_path_buf());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers.max(1))
        .build()
        .map_err(crate::error::value_error)?;
    pool.install(|| {
        rayon::scope(|scope| {
            for instance in instances {
                let config = config.clone();
                let output_dir = output_dir.clone();
                let progress = progress.clone();
                scope.spawn(move |_| {
                    if let Err(error) = process_instance(instance, &output_dir, &config, &progress)
                    {
                        logging::error(error);
                    }
                });
            }
        });
    });
    progress.lock().unwrap().print_report();
    Ok(())
}

pub fn build_config(
    specs: &[String],
    model: Option<&str>,
    model_class: Option<&str>,
    environment_class: Option<&str>,
) -> Result<Json> {
    let mut configs = specs
        .iter()
        .map(|s| crate::config::get_config_from_spec(s))
        .collect::<Result<Vec<_>>>()?;
    let mut overrides = Map::new();
    let mut model_config = Map::new();
    if let Some(model) = model {
        model_config.insert("model_name".into(), json!(model));
    }
    if let Some(class) = model_class {
        model_config.insert("model_class".into(), json!(class));
    }
    if let Some(class) = environment_class {
        overrides.insert("environment".into(), json!({"environment_class":class}));
    }
    if !model_config.is_empty() {
        overrides.insert("model".into(), Value::Object(model_config));
    }
    configs.push(Value::Object(overrides));
    Ok(recursive_merge(configs))
}

#[allow(dead_code)]
fn _merge(_: Option<Map>) {}
