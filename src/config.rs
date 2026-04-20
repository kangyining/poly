use anyhow::{bail, Result};
use chrono::NaiveDate;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(author, version, about)]
pub struct Cli {
    #[arg(long, env = "FORECAST_TARGET_DATE")]
    pub target_date: Option<NaiveDate>,

    #[arg(long, env = "CITY", default_value = "nyc")]
    pub city: String,

    #[arg(long, env = "STATION_CODE", default_value = "KLGA")]
    pub station_code: String,

    #[arg(long, env = "LAT", default_value_t = 40.7769)]
    pub lat: f64,

    #[arg(long, env = "LON", default_value_t = -73.8740)]
    pub lon: f64,

    #[arg(long, env = "FORECAST_HIGH_F")]
    pub forecast_high_f: Option<f64>,

    #[arg(
        long,
        env = "POLYMARKET_GAMMA_URL",
        default_value = "https://gamma-api.polymarket.com"
    )]
    pub gamma_url: String,

    #[arg(
        long,
        env = "POLYMARKET_CLOB_URL",
        default_value = "https://clob.polymarket.com"
    )]
    pub clob_url: String,

    #[arg(long, env = "MIN_EV_THRESHOLD", default_value_t = 0.02)]
    pub min_ev_threshold: f64,

    #[arg(long, env = "DESIRED_SHARES", default_value_t = 10.0)]
    pub desired_shares: f64,

    #[arg(long, env = "ORDER_SIZE_USD", default_value_t = 5.0)]
    pub order_size_usd: f64,

    #[arg(long, env = "MAX_TOTAL_STAKE_USD", default_value_t = 50.0)]
    pub max_total_stake_usd: f64,

    #[arg(long, env = "MAX_OPEN_POSITIONS", default_value_t = 8)]
    pub max_open_positions: usize,

    #[arg(
        long,
        env = "CALIBRATION_PATH",
        default_value = "../poly-weather/calibration.json"
    )]
    pub calibration_path: PathBuf,

    #[arg(
        long,
        env = "OPEN_POSITIONS_PATH",
        default_value = "../poly-weather/open_positions.json"
    )]
    pub open_positions_path: PathBuf,

    #[arg(
        long,
        env = "EXECUTION_PLAN_DIR",
        default_value = "../poly-weather"
    )]
    pub execution_plan_dir: PathBuf,

    #[arg(
        long,
        env = "EXECUTE_SCRIPT",
        default_value = "../poly-weather/execute.py"
    )]
    pub execute_script: PathBuf,

    #[arg(
        long,
        env = "LOG_DIRECTORY",
        default_value = "/tmp/polymarket-weather-bot"
    )]
    pub log_directory: PathBuf,

    #[arg(long, env = "DRY_RUN", default_value_t = true)]
    pub dry_run: bool,

    #[arg(long, env = "LOOP", default_value_t = false)]
    pub run_loop: bool,

    #[arg(long, env = "POLL_INTERVAL_SECS", default_value_t = 300)]
    pub poll_interval_secs: u64,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub target_date: Option<NaiveDate>,
    pub city: String,
    pub station_code: String,
    pub lat: f64,
    pub lon: f64,
    pub forecast_high_f: Option<f64>,
    pub gamma_url: String,
    pub clob_url: String,
    pub min_ev_threshold: f64,
    pub desired_shares: f64,
    pub order_size_usd: f64,
    pub max_total_stake_usd: f64,
    pub max_open_positions: usize,
    pub calibration_path: PathBuf,
    pub open_positions_path: PathBuf,
    pub execution_plan_dir: PathBuf,
    pub execute_script: PathBuf,
    pub log_directory: PathBuf,
    pub dry_run: bool,
    pub run_loop: bool,
    pub poll_interval_secs: u64,
}

impl TryFrom<Cli> for AppConfig {
    type Error = anyhow::Error;

    fn try_from(c: Cli) -> Result<Self> {
        if c.desired_shares <= 0.0 {
            bail!("desired_shares must be positive");
        }
        if c.order_size_usd <= 0.0 {
            bail!("order_size_usd must be positive");
        }
        if c.max_total_stake_usd <= 0.0 {
            bail!("max_total_stake_usd must be positive");
        }
        if c.poll_interval_secs == 0 {
            bail!("poll_interval_secs must be positive");
        }

        Ok(Self {
            target_date: c.target_date,
            city: c.city,
            station_code: c.station_code,
            lat: c.lat,
            lon: c.lon,
            forecast_high_f: c.forecast_high_f,
            gamma_url: c.gamma_url,
            clob_url: c.clob_url,
            min_ev_threshold: c.min_ev_threshold,
            desired_shares: c.desired_shares,
            order_size_usd: c.order_size_usd,
            max_total_stake_usd: c.max_total_stake_usd,
            max_open_positions: c.max_open_positions,
            calibration_path: c.calibration_path,
            open_positions_path: c.open_positions_path,
            execution_plan_dir: c.execution_plan_dir,
            execute_script: c.execute_script,
            log_directory: c.log_directory,
            dry_run: c.dry_run,
            run_loop: c.run_loop,
            poll_interval_secs: c.poll_interval_secs,
        })
    }
}
