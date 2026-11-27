use clap::{Parser, Subcommand};
use mini_swe_agent::{
    Result,
    run::{global_config_file, init, load_global_config, utilities::config},
};

#[derive(Parser)]
#[command(name = "mini-extra", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    Setup,
    Set {
        key: Option<String>,
        value: Option<String>,
    },
    Unset {
        key: Option<String>,
    },
    Edit,
}

fn main() {
    init();
    if let Err(error) = execute(Cli::parse()) {
        mini_swe_agent::run::report_error(&error);
        std::process::exit(1);
    }
}

fn execute(cli: Cli) -> Result<()> {
    let Commands::Config { command } = cli.command;
    match command {
        ConfigCommands::Setup => config::setup(),
        ConfigCommands::Set { key, value } => {
            let key = match key {
                Some(k) => k,
                None => config::prompt("Enter the key to set: ", "")?,
            };
            let value = match value {
                Some(v) => v,
                None => config::prompt(&format!("Enter the value for {key}: "), "")?,
            };
            config::set_key(&global_config_file(), &key, &value)?;
            load_global_config();
            Ok(())
        }
        ConfigCommands::Unset { key } => {
            let key = match key {
                Some(k) => k,
                None => config::prompt("Enter the key to unset: ", "")?,
            };
            config::unset_key(&global_config_file(), &key)
        }
        ConfigCommands::Edit => config::edit(),
    }
}
