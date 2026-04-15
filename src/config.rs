use anyhow::{bail, Result};
use chrono::NaiveDate;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(author, version, about)]
pub struct Cli {
    #[arg(long, env = "FORECAST_TARGET_DATE")]
    pub target_date: NaiveDate,
    #[arg(long, env = "FORECAST_HIGH_F")]
    pub forecast_high_f: f64,
    #[arg(
        long,
        env = "POLYMARKET_EVENT_SLUG",
        default_value = "highest-temperature-in-nyc-on-april-16-2026"
    )]
    pub event_slug: String,
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
    #[arg(long, env = "PRICE_MIN", default_value_t = 0.90)]
    pub price_min: f64,
    #[arg(long, env = "PRICE_MAX", default_value_t = 0.99)]
    pub price_max: f64,
    #[arg(long, env = "DISTANCE_THRESHOLD_F", default_value_t = 4.0)]
    pub distance_threshold_f: f64,
    #[arg(long, env = "ORDER_SIZE_USD", default_value_t = 5.0)]
    pub order_size_usd: f64,
    #[arg(long, env = "DRY_RUN", default_value_t = true)]
    pub dry_run: bool,
    #[arg(
        long,
        env = "LOG_DIRECTORY",
        default_value = "/tmp/polymarket-weather-bot"
    )]
    pub log_directory: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub target_date: NaiveDate,
    pub event_slug: String,
    pub gamma_url: String,
    pub clob_url: String,
    pub price_band: PriceBand,
    pub distance_threshold_f: f64,
    pub order_size_usd: f64,
    pub dry_run: bool,
    pub forecast_high_f: f64,
    pub log_directory: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct PriceBand {
    pub min: f64,
    pub max: f64,
}

impl TryFrom<Cli> for AppConfig {
    type Error = anyhow::Error;

    fn try_from(value: Cli) -> Result<Self> {
        if value.price_min >= value.price_max {
            bail!("price_min must be smaller than price_max");
        }
        if !(0.0..=1.0).contains(&value.price_min) || !(0.0..=1.0).contains(&value.price_max) {
            bail!("price bounds must be between 0 and 1");
        }
        if value.distance_threshold_f < 0.0 {
            bail!("distance_threshold_f must be non-negative");
        }
        if value.order_size_usd <= 0.0 {
            bail!("order_size_usd must be positive");
        }

        Ok(Self {
            target_date: value.target_date,
            event_slug: value.event_slug,
            gamma_url: value.gamma_url,
            clob_url: value.clob_url,
            price_band: PriceBand {
                min: value.price_min,
                max: value.price_max,
            },
            distance_threshold_f: value.distance_threshold_f,
            order_size_usd: value.order_size_usd,
            dry_run: value.dry_run,
            forecast_high_f: value.forecast_high_f,
            log_directory: value.log_directory,
        })
    }
}
