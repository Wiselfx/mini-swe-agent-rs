pub fn init() {
    crate::run::load_global_config();
    if !crate::run::env_flag("MSWEA_SILENT_STARTUP", false) {
        println!(
            "This is mini-swe-agent version {}.\nCheck the v2 migration guide at https://klieret.short.gy/mini-v2-migration\nLoading global config from '{}'",
            crate::VERSION,
            crate::run::global_config_file().display()
        );
    }
    if crate::run::env_flag("MSWEA_QUIET", false) {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("error")
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info".into()),
            )
            .with_target(false)
            .without_time()
            .try_init();
    }
}

pub fn report_error(error: &dyn std::error::Error) {
    tracing::error!("{error}");
    if let Some(source) = error.source() {
        tracing::error!("Caused by: {source}");
    }
}
