use anyhow::Result;
use clap::Args;
use serde_json::json;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use crate::data::{
    backfills, forecast_recorder, market_recorder, metar_recorder, taf_nws_recorder,
};
use crate::db::Database;

#[derive(Debug, Clone, Args)]
pub struct RecorderArgs {
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long)]
    pub once: bool,
    #[arg(long)]
    pub backfill: bool,
    #[arg(long)]
    pub backfill_us_high_temp_data: bool,
    #[arg(long)]
    pub backfill_forecasts: bool,
    #[arg(long)]
    pub backfill_prices: bool,
    #[arg(long)]
    pub backfill_settlement_temps: bool,
    #[arg(long)]
    pub backfill_metars: bool,
    #[arg(long)]
    pub backfill_missing_bins: bool,
    #[arg(long)]
    pub backfill_true_forecasts: bool,
    #[arg(long)]
    pub city: Option<String>,
    #[arg(long, default_value_t = 60)]
    pub days_back: i64,
    #[arg(long, default_value_t = 300)]
    pub interval_seconds: u64,
}

pub fn run(args: RecorderArgs) -> Result<()> {
    let db_path = args
        .db
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    let db = Database::open(&db_path, false)?;

    if args.backfill
        || args.backfill_us_high_temp_data
        || args.backfill_forecasts
        || args.backfill_prices
        || args.backfill_settlement_temps
        || args.backfill_metars
        || args.backfill_missing_bins
        || args.backfill_true_forecasts
    {
        let settlement_temps =
            if args.backfill || args.backfill_settlement_temps || args.backfill_us_high_temp_data {
                Some(backfills::backfill_settlement_temps(
                    &db,
                    args.city.as_deref(),
                    args.days_back,
                )?)
            } else {
                None
            };
        let historical_forecasts = if args.backfill || args.backfill_forecasts {
            Some(backfills::backfill_historical_forecasts(
                &db,
                args.city.as_deref(),
            )?)
        } else {
            None
        };
        let true_forecasts = if args.backfill || args.backfill_true_forecasts {
            Some(backfills::backfill_true_forecasts(
                &db,
                args.city.as_deref(),
            )?)
        } else {
            None
        };
        let metars = if args.backfill || args.backfill_metars {
            Some(backfills::backfill_asos_metars(
                &db,
                args.city.as_deref(),
                args.days_back,
            )?)
        } else {
            None
        };
        let prices = if args.backfill || args.backfill_prices {
            Some(backfills::backfill_price_history(
                &db,
                args.city.as_deref(),
            )?)
        } else {
            None
        };
        let missing_bins = if args.backfill || args.backfill_missing_bins {
            Some(backfills::backfill_missing_bins(&db, args.city.as_deref())?)
        } else {
            None
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "db": db_path,
                "settlement_temps": settlement_temps,
                "historical_forecasts": historical_forecasts,
                "true_forecasts": true_forecasts,
                "metars": metars,
                "prices": prices,
                "missing_bins": missing_bins,
            }))?
        );
        return Ok(());
    }

    if args.once {
        let run_summary = run_once(&db)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "db": db_path,
                "run": run_summary,
            }))?
        );
        return Ok(());
    }

    loop {
        let run_summary = run_once(&db)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "db": db_path,
                "run": run_summary,
                "next_run_seconds": args.interval_seconds,
            }))?
        );
        thread::sleep(Duration::from_secs(args.interval_seconds.max(1)));
    }
}

fn run_once(db: &Database) -> Result<serde_json::Value> {
    let metar_summary = metar_recorder::record_metars(db)?;
    let taf_summary = taf_nws_recorder::record_tafs(db)?;
    let nws_summary = taf_nws_recorder::record_nws_forecasts(db)?;
    let market_summary = market_recorder::record_market_snapshots(db)?;
    let forecast_summary = forecast_recorder::record_forecasts(db)?;
    Ok(json!({
        "metars": metar_summary,
        "tafs": taf_summary,
        "nws_forecasts": nws_summary,
        "market_snapshots": market_summary,
        "forecasts": forecast_summary,
    }))
}
