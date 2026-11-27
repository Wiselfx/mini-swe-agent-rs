use std::{fs::OpenOptions, io::Write, path::Path};
use tracing::{info, warn};

pub fn info(message: impl AsRef<str>) {
    info!("{}", message.as_ref());
}
pub fn warning(message: impl AsRef<str>) {
    warn!("{}", message.as_ref());
}
pub fn error(message: impl std::fmt::Display) {
    tracing::error!("{message}");
}

pub fn add_file_handler(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init()
        .ok();
    Ok(())
}

pub fn _write(_: &mut dyn Write) {}
