use anyhow::{bail, Result};
use chrono::{Duration, NaiveDate, Utc};
use clap::Args;
use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::analysis::{backtest, calibration, replay};
use crate::config;
use crate::db::Database;
use crate::strategy::registry;

#[derive(Debug, Clone, Args)]
pub struct BacktestArgs {
    #[arg(long, default_value = "base_ev")]
    pub strategy: String,
    #[arg(long)]
    pub strategy_group: Option<String>,
    #[arg(long)]
    pub all_strategies: bool,
    #[arg(long)]
    pub list_strategies: bool,
    #[arg(long, default_value_t = 60)]
    pub days: i64,
    #[arg(long)]
    pub date_from: Option<String>,
    #[arg(long)]
    pub date_to: Option<String>,
    #[arg(long)]
    pub city: Option<String>,
    #[arg(long, default_value = crate::DEFAULT_STRATEGIES_PATH)]
    pub strategies_file: PathBuf,
    #[arg(long, default_value = crate::DEFAULT_CALIBRATION_PATH)]
    pub calibration_file: PathBuf,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long = "csv")]
    pub summary_csv: Option<PathBuf>,
    #[arg(long)]
    pub trades_csv: Option<PathBuf>,
    #[arg(long)]
    pub report: Option<PathBuf>,
}

pub fn run(args: BacktestArgs) -> Result<()> {
    if args.list_strategies {
        println!(
            "{}",
            serde_json::to_string_pretty(&registry::list_strategy_configs(&args.strategies_file)?)?
        );
        return Ok(());
    }
    let today = Utc::now().date_naive();
    let date_to = parse_date_or(args.date_to.as_deref(), today - Duration::days(1))?;
    let date_from = if let Some(raw) = args.date_from.as_deref() {
        NaiveDate::parse_from_str(raw, "%Y-%m-%d")?
    } else {
        date_to - Duration::days(args.days - 1)
    };
    let city = config::normalize_city(args.city.as_deref());
    if let Some(city) = &city {
        let known = config::known_city_names();
        if !known.iter().any(|name| name == city) {
            bail!("--city must be one of: {}", known.join(", "));
        }
    }
    let strategy_names = if args.all_strategies {
        registry::all_members(&args.strategies_file)?
    } else if let Some(group) = args.strategy_group.as_deref() {
        registry::group_members(&args.strategies_file, group)?
    } else {
        vec![args.strategy.clone()]
    };
    let explicit_db = args.db.is_some();
    let db_path = args
        .db
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    if !explicit_db {
        seed_research_sources_if_present(&db_path)?;
    }
    let db = Database::open(&db_path, true)?;
    let calibration_table = calibration::load(&args.calibration_file)?;
    let dataset = replay::ReplayDataset::load(
        &db,
        date_from,
        date_to,
        city.as_deref(),
        backtest::DEFAULT_INTERVAL_MINUTES,
    )?;
    let auto_paths =
        backtest::auto_artifact_paths(&strategy_names, city.as_deref(), date_from, date_to);
    let summary_csv = args
        .summary_csv
        .unwrap_or_else(|| auto_paths["summary_csv"].clone());
    let trades_csv = args
        .trades_csv
        .unwrap_or_else(|| auto_paths["trades_csv"].clone());
    let report = args.report.unwrap_or_else(|| auto_paths["report"].clone());

    let mut summaries = Vec::new();
    let mut trades = Vec::new();
    for strategy_name in &strategy_names {
        let params = registry::load_strategy_params(&args.strategies_file, strategy_name)?;
        let summary = backtest::run_backtest_with_dataset(
            &dataset,
            backtest::BacktestRequest {
                strategy_name: strategy_name.clone(),
                params,
                date_from,
                date_to,
                city: city.clone(),
                calibration_table: calibration_table.clone(),
                include_trades: true,
            },
        )?;
        trades.extend(summary.trades.clone());
        summaries.push(summary);
    }
    backtest::write_summary_csv(&summary_csv, &summaries)?;
    backtest::write_trades_csv(&trades_csv, &trades)?;
    backtest::write_summary_report(&report, &summaries, &summary_csv, &trades_csv)?;
    let mut payload = if summaries.len() == 1 {
        serde_json::to_value(&summaries[0])?
    } else {
        serde_json::json!({ "runs": summaries })
    };
    if let Value::Object(obj) = &mut payload {
        obj.insert(
            "artifacts".to_string(),
            serde_json::json!({
                "summary_csv": summary_csv,
                "trades_csv": trades_csv,
                "report": report,
            }),
        );
    }
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn parse_date_or(raw: Option<&str>, default: NaiveDate) -> Result<NaiveDate> {
    Ok(match raw {
        Some(raw) => NaiveDate::parse_from_str(raw, "%Y-%m-%d")?,
        None => default,
    })
}

fn seed_research_sources_if_present(db_path: &Path) -> Result<()> {
    let path = Path::new("analysis/research_sources.json");
    if !path.exists() {
        return Ok(());
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(());
    };
    let Ok(sources) = serde_json::from_str::<Vec<Value>>(&text) else {
        return Ok(());
    };
    let db = Database::open(db_path, false)?;
    let _ = crate::db::seed_research_sources(&db, &sources);
    Ok(())
}
