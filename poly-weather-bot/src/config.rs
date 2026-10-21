use anyhow::{bail, Context, Result};
use chrono::{Duration, NaiveDate, Utc};
use clap::Parser;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::weather::market_close_utc;

#[derive(Debug, Clone, Parser)]
#[command(author, version, about)]
pub struct Cli {
    #[arg(long, default_value = "config.toml")]
    pub config: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StrategyConfig {
    pub name: String,
    #[serde(default = "bool_true")]
    pub enabled: bool,
    #[serde(default = "one")]
    pub funding_pct: f64,
    pub min_ev_threshold: f64,
    pub max_stake_usd: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CityConfig {
    pub slug: String,
    pub station_code: String,
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub forecast_high_f: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub start_date: Option<NaiveDate>,
    pub days_in_future: u32,
    pub cities: Vec<CityConfig>,
    pub strategies: Vec<StrategyConfig>,
    pub api: ApiConfig,
    pub paths: PathsConfig,
    pub trading: TradingConfig,
    pub runtime: RuntimeConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    pub gamma_url: String,
    pub clob_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PathsConfig {
    pub calibration_path: PathBuf,
    pub open_positions_path: PathBuf,
    pub execution_plan_dir: PathBuf,
    pub paper_portfolio_path: PathBuf,
    pub performance_csv_path: PathBuf,
    pub log_directory: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TradingConfig {
    pub dry_run: bool,
    pub max_total_stake_usd: f64,
    pub max_open_positions: usize,
    pub paper_starting_balance_usd: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    pub run_loop: bool,
    pub poll_interval_secs: u64,
    pub forecast_refresh_secs: u64,
}

#[derive(Debug, Clone)]
pub struct MarketContext {
    pub key: String,
    pub city_slug: String,
    pub station_code: String,
    pub lat: f64,
    pub lon: f64,
    pub forecast_high_f: Option<f64>,
    pub target_date: NaiveDate,
    pub event_slug: String,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config: Self = toml::from_str(&text)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn contexts(&self) -> Vec<MarketContext> {
        let dates = self.target_dates();
        let mut contexts = Vec::with_capacity(self.cities.len() * dates.len());
        for city in &self.cities {
            for target_date in &dates {
                let event_slug = build_event_slug(&city.slug, *target_date);
                contexts.push(MarketContext {
                    key: context_key(&city.slug, *target_date),
                    city_slug: city.slug.clone(),
                    station_code: city.station_code.clone(),
                    lat: city.lat,
                    lon: city.lon,
                    forecast_high_f: city.forecast_high_f,
                    target_date: *target_date,
                    event_slug,
                });
            }
        }
        contexts
    }

    pub fn context_map(&self) -> HashMap<String, MarketContext> {
        self.contexts()
            .into_iter()
            .map(|context| (context.key.clone(), context))
            .collect()
    }

    pub fn active_strategies(&self) -> impl Iterator<Item = &StrategyConfig> {
        self.strategies.iter().filter(|strategy| strategy.enabled)
    }

    pub fn active_strategy_count(&self) -> usize {
        self.active_strategies().count()
    }

    fn target_dates(&self) -> Vec<NaiveDate> {
        let start = resolve_start_date(self.start_date);
        (0..self.days_in_future)
            .map(|offset| start + Duration::days(i64::from(offset)))
            .collect()
    }

    fn validate(&self) -> Result<()> {
        if self.days_in_future == 0 {
            bail!("days_in_future must be positive");
        }
        if self.cities.is_empty() {
            bail!("cities must not be empty");
        }
        if self.trading.max_total_stake_usd <= 0.0 {
            bail!("max_total_stake_usd must be positive");
        }
        if self.strategies.is_empty() {
            bail!("strategies must not be empty");
        }
        if self.trading.paper_starting_balance_usd <= 0.0 {
            bail!("paper_starting_balance_usd must be positive");
        }
        if self.runtime.poll_interval_secs == 0 {
            bail!("poll_interval_secs must be positive");
        }
        if self.runtime.forecast_refresh_secs == 0 {
            bail!("forecast_refresh_secs must be positive");
        }

        for city in &self.cities {
            if city.slug.trim().is_empty() {
                bail!("city slug must not be empty");
            }
            if city.station_code.trim().is_empty() {
                bail!("city station_code must not be empty");
            }
        }

        let enabled_count = self.active_strategy_count();
        if enabled_count == 0 {
            bail!("at least one strategy must be enabled");
        }

        for strategy in &self.strategies {
            if strategy.name.trim().is_empty() {
                bail!("strategy name must not be empty");
            }
            if strategy.funding_pct <= 0.0 {
                bail!("strategy funding_pct must be positive");
            }
            if strategy.min_ev_threshold <= 0.0 {
                bail!("strategy min_ev_threshold must be positive");
            }
            if strategy.max_stake_usd <= 0.0 {
                bail!("strategy max_stake_usd must be positive");
            }
        }

        Ok(())
    }
}

pub fn resolve_start_date(override_date: Option<NaiveDate>) -> NaiveDate {
    if let Some(date) = override_date {
        return date;
    }

    let now = Utc::now();
    let today = now.date_naive();
    if now < market_close_utc(today) {
        today
    } else {
        today + Duration::days(1)
    }
}

pub fn context_key(city_slug: &str, target_date: NaiveDate) -> String {
    format!("{city_slug}:{}", target_date.format("%Y-%m-%d"))
}

pub fn build_event_slug(city: &str, date: NaiveDate) -> String {
    let month = match date.month() {
        1 => "january",
        2 => "february",
        3 => "march",
        4 => "april",
        5 => "may",
        6 => "june",
        7 => "july",
        8 => "august",
        9 => "september",
        10 => "october",
        11 => "november",
        12 => "december",
        _ => "unknown",
    };

    format!(
        "highest-temperature-in-{}-on-{}-{}-{}",
        city,
        month,
        date.day(),
        date.year()
    )
}

use chrono::Datelike;

fn bool_true() -> bool {
    true
}

fn one() -> f64 {
    1.0
}
