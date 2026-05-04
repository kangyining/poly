use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use clap::Args;
use serde_json::json;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use crate::data::{
    backfills, forecast_recorder, market_recorder, metar_recorder, settlement_tracker,
    taf_nws_recorder, weather_contracts,
};
use crate::db;
use crate::db::Database;

#[derive(Debug, Clone, Args)]
pub struct RecorderArgs {
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long, default_value = "open_positions.json")]
    pub positions: PathBuf,
    #[arg(long)]
    pub once: bool,
    #[arg(long)]
    pub discover_contracts: bool,
    #[arg(long)]
    pub broad_gamma_discovery: bool,
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
    #[arg(long, default_value_t = 30)]
    pub interval_seconds: u64,
}

pub fn run(args: RecorderArgs) -> Result<()> {
    let db_path = args
        .db
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    let db = Database::open(&db_path, false)?;

    if args.discover_contracts {
        let discovery_summary =
            discover_contracts(&db, args.broad_gamma_discovery, args.city.as_deref())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "db": db_path,
                "contract_discovery": discovery_summary,
            }))?
        );
        return Ok(());
    }

    if args.backfill
        || args.backfill_us_high_temp_data
        || args.backfill_forecasts
        || args.backfill_prices
        || args.backfill_settlement_temps
        || args.backfill_metars
        || args.backfill_missing_bins
        || args.backfill_true_forecasts
    {
        if args.backfill {
            let settlements =
                settlement_tracker::backfill(&db, args.days_back, args.city.as_deref())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "db": db_path,
                    "settlements": settlements,
                }))?
            );
            return Ok(());
        }
        let settlement_backfill = if args.backfill_us_high_temp_data {
            Some(settlement_tracker::backfill(
                &db,
                args.days_back,
                args.city.as_deref(),
            )?)
        } else {
            None
        };
        let settlement_temps = if args.backfill_settlement_temps || args.backfill_us_high_temp_data
        {
            Some(backfills::backfill_settlement_temps(
                &db,
                args.city.as_deref(),
                args.days_back,
            )?)
        } else {
            None
        };
        let historical_forecasts = if args.backfill_forecasts || args.backfill_us_high_temp_data {
            Some(backfills::backfill_historical_forecasts(
                &db,
                args.city.as_deref(),
            )?)
        } else {
            None
        };
        let true_forecasts = if args.backfill_true_forecasts || args.backfill_us_high_temp_data {
            Some(backfills::backfill_true_forecasts(
                &db,
                args.city.as_deref(),
            )?)
        } else {
            None
        };
        let metars = if args.backfill_metars || args.backfill_us_high_temp_data {
            Some(backfills::backfill_asos_metars(
                &db,
                args.city.as_deref(),
                args.days_back,
            )?)
        } else {
            None
        };
        let prices = if args.backfill_prices || args.backfill_us_high_temp_data {
            Some(backfills::backfill_price_history(
                &db,
                args.city.as_deref(),
            )?)
        } else {
            None
        };
        let missing_bins = if args.backfill_missing_bins || args.backfill_us_high_temp_data {
            Some(backfills::backfill_missing_bins(&db, args.city.as_deref())?)
        } else {
            None
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "db": db_path,
                "settlements": settlement_backfill,
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
        let run_summary = run_once(&db, args.broad_gamma_discovery, &args.positions)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "db": db_path,
                "run": run_summary,
            }))?
        );
        return Ok(());
    }

    let first_run = run_once(&db, args.broad_gamma_discovery, &args.positions)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "db": db_path,
            "run": first_run,
            "next_run_seconds": args.interval_seconds,
        }))?
    );
    let mut scheduler = SchedulerState::just_ran(Utc::now());
    loop {
        thread::sleep(Duration::from_secs(args.interval_seconds.max(1)));
        let now = Utc::now();
        let due = due_jobs(now, &scheduler);
        if due.is_empty() {
            continue;
        }
        let run_summary = run_due(
            &db,
            due.clone(),
            now,
            args.broad_gamma_discovery,
            &args.positions,
        )?;
        scheduler.record(due, now);
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "db": db_path,
                "run": run_summary,
                "next_run_seconds": args.interval_seconds,
            }))?
        );
    }
}

fn run_once(
    db: &Database,
    broad_gamma_discovery: bool,
    positions_path: &std::path::Path,
) -> Result<serde_json::Value> {
    let metar_summary = metar_recorder::record_metars(db)?;
    let taf_summary = taf_nws_recorder::record_tafs(db)?;
    let nws_summary = taf_nws_recorder::record_nws_forecasts(db)?;
    let market_summary = record_market_snapshots(db, broad_gamma_discovery)?;
    let forecast_summary = forecast_recorder::record_forecasts(db)?;
    let settlement_summary = settlement_tracker::daily_check(db, None, Some(positions_path))?;
    Ok(json!({
        "metars": metar_summary,
        "tafs": taf_summary,
        "nws_forecasts": nws_summary,
        "market_snapshots": market_summary,
        "forecasts": forecast_summary,
        "settlements": settlement_summary,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecorderJob {
    Metars,
    Tafs,
    MarketSnapshots,
    Forecasts,
    NwsForecasts,
    Settlements,
}

#[derive(Debug, Clone)]
struct SchedulerState {
    metars: DateTime<Utc>,
    tafs: DateTime<Utc>,
    market_snapshots: DateTime<Utc>,
    forecasts: DateTime<Utc>,
    nws_forecasts: DateTime<Utc>,
    settlement_day: chrono::NaiveDate,
}

impl SchedulerState {
    fn just_ran(now: DateTime<Utc>) -> Self {
        Self {
            metars: now,
            tafs: now,
            market_snapshots: now,
            forecasts: now,
            nws_forecasts: now,
            settlement_day: now.date_naive(),
        }
    }

    fn record(&mut self, due: Vec<RecorderJob>, now: DateTime<Utc>) {
        for job in due {
            match job {
                RecorderJob::Metars => self.metars = now,
                RecorderJob::Tafs => self.tafs = now,
                RecorderJob::MarketSnapshots => self.market_snapshots = now,
                RecorderJob::Forecasts => self.forecasts = now,
                RecorderJob::NwsForecasts => self.nws_forecasts = now,
                RecorderJob::Settlements => self.settlement_day = now.date_naive(),
            }
        }
    }
}

fn due_jobs(now: DateTime<Utc>, state: &SchedulerState) -> Vec<RecorderJob> {
    let mut due = Vec::new();
    if now
        .signed_duration_since(state.market_snapshots)
        .num_minutes()
        >= 5
    {
        due.push(RecorderJob::MarketSnapshots);
    }
    if now.signed_duration_since(state.metars).num_minutes() >= 60 {
        due.push(RecorderJob::Metars);
    }
    if now.signed_duration_since(state.tafs).num_minutes() >= 60 {
        due.push(RecorderJob::Tafs);
    }
    if now.signed_duration_since(state.forecasts).num_minutes() >= 60 {
        due.push(RecorderJob::Forecasts);
    }
    if now.signed_duration_since(state.nws_forecasts).num_minutes() >= 30 {
        due.push(RecorderJob::NwsForecasts);
    }
    if now.hour() >= 6 && state.settlement_day != now.date_naive() {
        due.push(RecorderJob::Settlements);
    }
    due
}

fn run_due(
    db: &Database,
    due: Vec<RecorderJob>,
    now: DateTime<Utc>,
    broad_gamma_discovery: bool,
    positions_path: &std::path::Path,
) -> Result<serde_json::Value> {
    let mut out = serde_json::Map::new();
    for job in due {
        match job {
            RecorderJob::Metars => {
                out.insert(
                    "metars".to_string(),
                    serde_json::to_value(metar_recorder::record_metars(db)?)?,
                );
            }
            RecorderJob::Tafs => {
                out.insert(
                    "tafs".to_string(),
                    serde_json::to_value(taf_nws_recorder::record_tafs(db)?)?,
                );
            }
            RecorderJob::MarketSnapshots => {
                out.insert(
                    "market_snapshots".to_string(),
                    serde_json::to_value(record_market_snapshots(db, broad_gamma_discovery)?)?,
                );
            }
            RecorderJob::Forecasts => {
                out.insert(
                    "forecasts".to_string(),
                    serde_json::to_value(forecast_recorder::record_forecasts(db)?)?,
                );
            }
            RecorderJob::NwsForecasts => {
                out.insert(
                    "nws_forecasts".to_string(),
                    serde_json::to_value(taf_nws_recorder::record_nws_forecasts(db)?)?,
                );
            }
            RecorderJob::Settlements => {
                let target = now.date_naive() - chrono::Duration::days(1);
                out.insert(
                    "settlements".to_string(),
                    serde_json::to_value(settlement_tracker::daily_check(
                        db,
                        Some(target),
                        Some(positions_path),
                    )?)?,
                );
            }
        }
    }
    Ok(serde_json::Value::Object(out))
}

fn record_market_snapshots(
    db: &Database,
    broad_gamma_discovery: bool,
) -> Result<market_recorder::SnapshotRunSummary> {
    if broad_gamma_discovery {
        market_recorder::record_market_snapshots_with_mode(
            db,
            market_recorder::DiscoveryMode::BroadGamma,
        )
    } else {
        market_recorder::record_market_snapshots(db)
    }
}

fn discover_contracts(
    db: &Database,
    broad_gamma_discovery: bool,
    city: Option<&str>,
) -> Result<serde_json::Value> {
    let now = Utc::now();
    let discoveries = if broad_gamma_discovery {
        weather_contracts::discover_weather_contracts(None, true)?
    } else {
        weather_contracts::discover_configured_weather_contracts(
            &market_recorder::dates_to_record(now),
            true,
        )
    };
    let city = city.map(|value| value.to_lowercase());
    let mut seen = 0usize;
    let mut valid = 0usize;
    let mut skipped = 0usize;
    for discovery in discoveries {
        if let Some(city) = &city {
            if discovery.contract.city_slug.to_lowercase() != *city
                && discovery.contract.display_city.to_lowercase() != *city
            {
                continue;
            }
        }
        seen += 1;
        if discovery.is_valid() {
            valid += 1;
        } else {
            skipped += 1;
        }
        db::upsert_weather_contract(db, &discovery.contract)?;
    }
    Ok(json!({
        "mode": if broad_gamma_discovery { "broad_gamma" } else { "configured" },
        "city": city,
        "contracts_seen": seen,
        "contracts_valid": valid,
        "contracts_skipped": skipped,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn scheduler_uses_python_compatible_cadences() {
        let start = Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();
        let state = SchedulerState::just_ran(start);
        assert_eq!(
            due_jobs(start + chrono::Duration::minutes(5), &state),
            vec![RecorderJob::MarketSnapshots]
        );
        let due = due_jobs(start + chrono::Duration::minutes(30), &state);
        assert!(due.contains(&RecorderJob::MarketSnapshots));
        assert!(due.contains(&RecorderJob::NwsForecasts));
        let due = due_jobs(start + chrono::Duration::hours(6), &state);
        assert!(due.contains(&RecorderJob::Metars));
        assert!(due.contains(&RecorderJob::Tafs));
        assert!(due.contains(&RecorderJob::Forecasts));
    }
}
