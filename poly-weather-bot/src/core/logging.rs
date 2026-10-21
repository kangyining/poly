use std::fs::OpenOptions;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Local;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init_logging(log_directory: &Path) -> Result<WorkerGuard> {
    std::fs::create_dir_all(log_directory).with_context(|| {
        format!(
            "failed to create log directory at {}",
            log_directory.display()
        )
    })?;

    let started_at = Local::now().format("%Y%m%d-%H%M%S");
    let log_path = log_directory.join(format!("trading-bot-{started_at}.log"));
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open log file at {}", log_path.display()))?;
    let (file_writer, guard) = tracing_appender::non_blocking(log_file);
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            fmt::layer()
                .with_writer(std::io::stdout)
                .with_target(false)
                .with_span_events(FmtSpan::CLOSE),
        )
        .with(
            fmt::layer()
                .json()
                .with_writer(file_writer)
                .with_current_span(true)
                .with_span_list(true),
        )
        .init();

    Ok(guard)
}
