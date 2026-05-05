use anyhow::Result;
use clap::Parser;
use tracing::warn;

use polymarket_weather_bot::app::App;
use polymarket_weather_bot::config::{AppConfig, Cli};
use polymarket_weather_bot::core::logging::init_logging;
use polymarket_weather_bot::strategy::CalibrationTable;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let config = AppConfig::load(&cli.config)?;
    let _logging_guard = init_logging(&config.log_directory)?;

    let calibration = if config.calibration_path.exists() {
        match CalibrationTable::load(&config.calibration_path) {
            Ok(table) => Some(table),
            Err(error) => {
                warn!(error = %error, "failed to load calibration, using defaults");
                None
            }
        }
    } else {
        warn!(
            path = %config.calibration_path.display(),
            "calibration.json not found, using defaults"
        );
        None
    };

    App::new(config, calibration).run().await
}
