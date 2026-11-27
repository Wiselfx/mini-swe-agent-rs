use clap::Parser;
use mini_swe_agent::{
    Result,
    run::{
        benchmarks::{
            progress::filter_instances,
            swebench::{build_config, load_dataset, run_batch},
        },
        init,
    },
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "swebench",
    version,
    about = "Run mini-SWE-agent on SWE-bench instances in batch mode."
)]
struct Args {
    #[arg(long, default_value = "lite")]
    subset: String,
    #[arg(long, default_value = "dev")]
    split: String,
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
    #[arg(short = 'c', long = "config", default_values = ["swebench.yaml"])]
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
    let output = PathBuf::from(if args.output.is_empty() {
        "."
    } else {
        &args.output
    });
    let instances = load_dataset(&args.subset, &args.split)?;
    let instances = filter_instances(instances, &args.filter, &args.slice, args.shuffle);
    let config = build_config(
        &args.config,
        args.model.as_deref(),
        args.model_class.as_deref(),
        args.environment_class.as_deref(),
    )?;
    run_batch(
        instances,
        &output,
        config,
        args.workers.max(1),
        args.redo_existing,
    )
}
