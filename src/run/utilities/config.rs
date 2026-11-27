use crate::{
    Json, Result,
    run::{env_flag, global_config_file, load_global_config},
};
use serde_json::Value;
use std::{env, io::Write, path::Path, process::Command};

pub fn prompt(label: &str, default: &str) -> Result<String> {
    print!("{label}");
    if !default.is_empty() {
        print!(" [{default}]");
    }
    print!(" ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(default.to_owned());
    }
    let line = line.trim();
    Ok(if line.is_empty() {
        default.to_owned()
    } else {
        line.to_owned()
    })
}

pub fn setup() -> Result<()> {
    println!(
        "To get started, we need to set up your global config file.\nYou can edit it manually or use `mini-extra config set KEY VALUE`.\nPopular models: anthropic/claude-sonnet-4-5, openai/gpt-5-mini, gemini/gemini-2.5-pro.\nYou can leave any setting blank to skip it."
    );
    let path = global_config_file();
    let default_model = env::var("MSWEA_MODEL_NAME").unwrap_or_default();
    let model = prompt("Enter your default model: ", &default_model)?;
    if !model.is_empty() {
        set_key(&path, "MSWEA_MODEL_NAME", &model)?;
    }
    let key_name = prompt("Enter your API key name (e.g., ANTHROPIC_API_KEY): ", "")?;
    if !key_name.is_empty() {
        let default = env::var(&key_name).unwrap_or_default();
        let value = prompt("Enter your API key value: ", &default)?;
        if !value.is_empty() {
            set_key(&path, &key_name, &value)?;
        }
    }
    set_key(&path, "MSWEA_CONFIGURED", "true")?;
    load_global_config();
    println!("Config finished. You can revisit it with `mini-extra config setup`.");
    Ok(())
}

pub fn configure_if_first_time() -> Result<()> {
    if !env_flag("MSWEA_CONFIGURED", false) {
        setup()?;
    }
    Ok(())
}

pub fn set_key(path: &Path, key: &str, value: &str) -> Result<()> {
    let mut lines = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let prefix = format!("{key}=");
    if let Some(line) = lines
        .iter_mut()
        .find(|l| l.trim_start().starts_with(&prefix))
    {
        *line = format!("{key}={value}");
    } else {
        lines.push(format!("{key}={value}"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, lines.join("\n") + "\n")?;
    unsafe {
        std::env::set_var(key, value);
    }
    Ok(())
}

pub fn unset_key(path: &Path, key: &str) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let prefix = format!("{key}=");
    let lines = text
        .lines()
        .filter(|l| !l.trim_start().starts_with(&prefix))
        .collect::<Vec<_>>();
    std::fs::write(path, lines.join("\n") + "\n")?;
    unsafe {
        env::remove_var(key);
    }
    Ok(())
}

pub fn edit() -> Result<()> {
    let editor = env::var("EDITOR").unwrap_or_else(|_| "nano".into());
    let status = Command::new(editor).arg(global_config_file()).status()?;
    if !status.success() {
        return Err(crate::error::value_error("Editor exited unsuccessfully"));
    }
    load_global_config();
    Ok(())
}

pub fn _types(_: Option<Json>, _: Option<Value>) {}
