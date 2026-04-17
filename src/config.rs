use anyhow::{bail, Result};
use chrono::NaiveDate;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub target_date: NaiveDate,
    pub event_slug: String,
    pub orderbook_sync_seconds: u64,
    pub max_book_staleness_seconds: i64,
    pub price_band: PriceBand,
    pub distance_threshold_f: f64,
    pub order_size_usd: f64,
    pub dry_run: bool,
    pub monitor_mode: bool,
    pub poll_interval_seconds: u64,
    pub log_directory: PathBuf,
    pub storage_path: PathBuf,
    pub polymarket_private_key: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct PriceBand {
    pub min: f64,
    pub max: f64,
}

impl AppConfig {
    pub fn new() -> Result<Self> {
        let config = Self {
            target_date: NaiveDate::from_ymd_opt(2026, 4, 16).expect("valid hardcoded date"),
            event_slug: "highest-temperature-in-nyc-on-april-16-2026".to_string(),
            orderbook_sync_seconds: 8,
            max_book_staleness_seconds: 30,
            price_band: PriceBand {
                min: 0.90,
                max: 0.99,
            },
            distance_threshold_f: 4.0,
            order_size_usd: 5.0,
            dry_run: true,
            monitor_mode: false,
            poll_interval_seconds: 60,
            log_directory: PathBuf::from("/tmp/polymarket-weather-bot"),
            storage_path: PathBuf::from("/tmp/polymarket-weather-bot/runs.jsonl"),
            polymarket_private_key: None,
        };

        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.price_band.min >= self.price_band.max {
            bail!("price_band.min must be smaller than price_band.max");
        }
        if !(0.0..=1.0).contains(&self.price_band.min)
            || !(0.0..=1.0).contains(&self.price_band.max)
        {
            bail!("price bounds must be between 0 and 1");
        }
        if self.distance_threshold_f < 0.0 {
            bail!("distance_threshold_f must be non-negative");
        }
        if self.order_size_usd <= 0.0 {
            bail!("order_size_usd must be positive");
        }
        if self.poll_interval_seconds == 0 {
            bail!("poll_interval_seconds must be positive");
        }
        if self.orderbook_sync_seconds == 0 {
            bail!("orderbook_sync_seconds must be positive");
        }
        if self.max_book_staleness_seconds <= 0 {
            bail!("max_book_staleness_seconds must be positive");
        }

        Ok(())
    }
}
