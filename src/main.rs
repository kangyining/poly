use anyhow::Result;
use polymarket_weather_bot::config::AppConfig;
use polymarket_weather_bot::logging::init_logging;
use polymarket_weather_bot::runner::run;
use polymarket_weather_bot::storage::JsonlStore;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::new()?;
    let _logging_guard = init_logging(&config.log_directory)?;
    let store = JsonlStore::new(&config.storage_path);

    run(&config, &store).await
}
