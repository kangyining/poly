use anyhow::Result;
use chrono::{Duration, NaiveDate, Utc};
use clap::Args;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

use crate::analysis::{backtest, bankroll, calibration};
use crate::config;
use crate::db::Database;
use crate::strategy::registry;

#[derive(Debug, Clone, Args)]
pub struct BankrollArgs {
    #[arg(long, default_value = "base_ev")]
    pub strategy: String,
    #[arg(long)]
    pub date_from: Option<String>,
    #[arg(long)]
    pub date_to: Option<String>,
    #[arg(long, default_value_t = 60)]
    pub days: i64,
    #[arg(long)]
    pub city: Option<String>,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long, default_value = crate::DEFAULT_STRATEGIES_PATH)]
    pub strategies_file: PathBuf,
    #[arg(long, default_value = crate::DEFAULT_CALIBRATION_PATH)]
    pub calibration_file: PathBuf,
    #[arg(long, default_value_t = 100.0)]
    pub initial_bankroll: f64,
    #[arg(long, default_value_t = 1.0)]
    pub min_trade_stake_usd: f64,
    #[arg(long)]
    pub allow_scale_down: bool,
    #[arg(long)]
    pub no_scale_down: bool,
    #[arg(long = "csv")]
    pub summary_csv: Option<PathBuf>,
    #[arg(long = "json")]
    pub summary_json: Option<PathBuf>,
    #[arg(long)]
    pub trades_csv: Option<PathBuf>,
    #[arg(long)]
    pub skips_csv: Option<PathBuf>,
    #[arg(long)]
    pub equity_csv: Option<PathBuf>,
    #[arg(long)]
    pub report: Option<PathBuf>,
}

pub fn run(args: BankrollArgs) -> Result<()> {
    let today = Utc::now().date_naive();
    let date_to = args
        .date_to
        .as_deref()
        .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
        .transpose()?
        .unwrap_or(today - Duration::days(1));
    let date_from = args
        .date_from
        .as_deref()
        .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
        .transpose()?
        .unwrap_or(date_to - Duration::days(args.days - 1));
    let city = config::normalize_city(args.city.as_deref());
    let db_path = args
        .db
        .clone()
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    let db = Database::open(&db_path, true)?;
    let params = registry::load_strategy_params(&args.strategies_file, &args.strategy)?;
    let calibration_table = calibration::load(&args.calibration_file)?;
    let summary = backtest::run_backtest(
        &db,
        backtest::BacktestRequest {
            strategy_name: args.strategy.clone(),
            params,
            date_from,
            date_to,
            city,
            calibration_table,
            include_trades: true,
        },
    )?;
    let candidates = summary
        .trades
        .iter()
        .map(|row| {
            bankroll::ReplayCandidate::from_trade_row(
                row,
                summary.city.as_deref(),
                &summary.strategy_plugin,
            )
        })
        .collect::<Vec<_>>();
    let allow_scale_down = if args.no_scale_down {
        false
    } else {
        args.allow_scale_down || !args.no_scale_down
    };
    let ledger = bankroll::apply_bankroll_ledger(
        candidates,
        args.initial_bankroll,
        args.min_trade_stake_usd,
        allow_scale_down,
    );
    let artifacts = auto_paths(&args, &summary);
    write_summary_json(
        args.summary_json.as_ref().unwrap_or(&artifacts.1),
        &summary,
        &ledger,
        allow_scale_down,
        args.min_trade_stake_usd,
    )?;
    write_summary_csv(
        args.summary_csv.as_ref().unwrap_or(&artifacts.0),
        &summary,
        &ledger,
        allow_scale_down,
        args.min_trade_stake_usd,
    )?;
    write_rows(
        args.trades_csv.as_ref().unwrap_or(&artifacts.2),
        &ledger.trades,
    )?;
    write_rows(
        args.skips_csv.as_ref().unwrap_or(&artifacts.3),
        &ledger.skips,
    )?;
    write_rows(
        args.equity_csv.as_ref().unwrap_or(&artifacts.4),
        &ledger.equity_curve,
    )?;
    write_report(
        args.report.as_ref().unwrap_or(&artifacts.5),
        &summary,
        &ledger,
        allow_scale_down,
        args.min_trade_stake_usd,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"summary": combined_summary(&summary, &ledger, allow_scale_down, args.min_trade_stake_usd), "artifacts": {
                "summary_csv": args.summary_csv.unwrap_or(artifacts.0),
                "summary_json": args.summary_json.unwrap_or(artifacts.1),
                "trades_csv": args.trades_csv.unwrap_or(artifacts.2),
                "skips_csv": args.skips_csv.unwrap_or(artifacts.3),
                "equity_csv": args.equity_csv.unwrap_or(artifacts.4),
                "report": args.report.unwrap_or(artifacts.5),
            }})
        )?
    );
    Ok(())
}

fn auto_paths(
    args: &BankrollArgs,
    summary: &backtest::BacktestSummary,
) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let city = summary
        .city
        .clone()
        .unwrap_or_else(|| "all".to_string())
        .to_lowercase()
        .replace(' ', "-");
    let stem = format!(
        "{}_{}_{}_bankroll_{}",
        city, summary.date_from, summary.date_to, args.strategy
    );
    (
        PathBuf::from(format!("backtest/bankroll/{}_summary.csv", stem)),
        PathBuf::from(format!("backtest/bankroll/{}_summary.json", stem)),
        PathBuf::from(format!("backtest/bankroll/{}_trades.csv", stem)),
        PathBuf::from(format!("backtest/bankroll/{}_skips.csv", stem)),
        PathBuf::from(format!("backtest/bankroll/{}_equity.csv", stem)),
        PathBuf::from(format!("backtest/bankroll/{}_report.md", stem)),
    )
}

fn write_summary_json(
    path: &PathBuf,
    summary: &backtest::BacktestSummary,
    ledger: &bankroll::LedgerResult,
    allow_scale_down: bool,
    min_trade_stake_usd: f64,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&combined_summary(
            summary,
            ledger,
            allow_scale_down,
            min_trade_stake_usd,
        ))?,
    )?;
    Ok(())
}

fn write_summary_csv(
    path: &PathBuf,
    summary: &backtest::BacktestSummary,
    ledger: &bankroll::LedgerResult,
    allow_scale_down: bool,
    min_trade_stake_usd: f64,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_maps(
        path,
        &[combined_summary(
            summary,
            ledger,
            allow_scale_down,
            min_trade_stake_usd,
        )],
        &[
            "backtest_id",
            "strategy",
            "strategy_plugin",
            "city",
            "date_from",
            "date_to",
            "backtest_days",
            "initial_bankroll",
            "final_equity",
            "portfolio_return",
            "realized_pnl",
            "avg_pnl_per_day",
            "max_drawdown",
            "max_locked_capital",
            "max_capital_utilization",
            "avg_capital_utilization",
            "trades_opened",
            "trades_scaled",
            "trades_skipped_insufficient_cash",
            "total_stake",
            "turnover_yield",
            "candidate_count",
            "days_considered",
            "days_with_forecast",
            "days_with_price_history",
        ],
    )
}

fn write_report(
    path: &PathBuf,
    summary: &backtest::BacktestSummary,
    ledger: &bankroll::LedgerResult,
    allow_scale_down: bool,
    min_trade_stake_usd: f64,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let combined = combined_summary(summary, ledger, allow_scale_down, min_trade_stake_usd);
    std::fs::write(
        path,
        format!(
            "# Bankroll Backtest Report: {}\n\n## Scope\n\n- Date range: `{}` to `{}`\n- Strategy: `{}` (`{}`)\n- Initial bankroll: `{}`\n- Min trade stake: `{}`\n- Scale down enabled: `{}`\n\n## Portfolio Metrics\n\n- Final equity: `{}`\n- Portfolio return: `{}`\n- Realized PnL: `{}`\n- Avg PnL/day: `{}`\n- Max drawdown: `{}`\n- Max locked capital: `{}`\n- Max capital utilization: `{}`\n- Avg capital utilization: `{}`\n\n## Trading Metrics\n\n- Trades opened: `{}`\n- Trades scaled: `{}`\n- Trades skipped insufficient cash: `{}`\n- Turnover yield: `{}`\n",
            combined.get("city").and_then(Value::as_str).unwrap_or("All Cities"),
            summary.date_from,
            summary.date_to,
            summary.strategy,
            summary.strategy_plugin,
            ledger.summary.initial_bankroll,
            min_trade_stake_usd,
            allow_scale_down,
            ledger.summary.final_equity,
            combined.get("portfolio_return").cloned().unwrap_or(Value::Null),
            ledger.summary.realized_pnl,
            combined.get("avg_pnl_per_day").cloned().unwrap_or(Value::Null),
            ledger.summary.max_drawdown,
            ledger.summary.max_locked_capital,
            ledger.summary.max_capital_utilization,
            ledger.summary.avg_capital_utilization,
            ledger.summary.trades_opened,
            ledger.summary.trades_scaled,
            ledger.summary.trades_skipped_insufficient_cash,
            combined.get("turnover_yield").cloned().unwrap_or(Value::Null)
        ),
    )?;
    Ok(())
}

fn combined_summary(
    summary: &backtest::BacktestSummary,
    ledger: &bankroll::LedgerResult,
    allow_scale_down: bool,
    min_trade_stake_usd: f64,
) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("backtest_id".into(), json!(summary.backtest_id));
    out.insert("strategy".into(), json!(summary.strategy));
    out.insert("strategy_plugin".into(), json!(summary.strategy_plugin));
    out.insert("params".into(), json!(summary.params));
    out.insert("city".into(), json!(summary.city));
    out.insert("date_from".into(), json!(summary.date_from));
    out.insert("date_to".into(), json!(summary.date_to));
    out.insert("backtest_days".into(), json!(summary.days_considered));
    out.insert("candidate_count".into(), json!(summary.total_trades));
    out.insert("days_considered".into(), json!(summary.days_considered));
    out.insert(
        "days_with_forecast".into(),
        json!(summary.days_with_forecast),
    );
    out.insert(
        "days_with_price_history".into(),
        json!(summary.days_with_price_history),
    );
    out.insert("min_trade_stake_usd".into(), json!(min_trade_stake_usd));
    out.insert("allow_scale_down".into(), json!(allow_scale_down));
    out.insert("result_storage".into(), json!("files"));

    let ledger_value = serde_json::to_value(&ledger.summary).unwrap_or(Value::Null);
    if let Value::Object(obj) = ledger_value {
        out.extend(obj);
    }
    let avg_pnl = if summary.days_considered > 0 {
        Some(ledger.summary.realized_pnl / summary.days_considered as f64)
    } else {
        None
    };
    out.insert("avg_pnl_per_day".into(), json!(avg_pnl));
    out
}

fn write_rows<T: Serialize>(path: &PathBuf, rows: &[T]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut writer = csv::Writer::from_path(path)?;
    for row in rows {
        writer.serialize(row)?;
    }
    writer.flush()?;
    Ok(())
}

fn write_maps(path: &PathBuf, rows: &[Map<String, Value>], preferred: &[&str]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut keys = preferred.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    for row in rows {
        for key in row.keys() {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }
    let mut writer = csv::Writer::from_path(path)?;
    writer.write_record(&keys)?;
    for row in rows {
        let record = keys
            .iter()
            .map(|key| {
                row.get(key)
                    .map(|value| match value {
                        Value::Null => String::new(),
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        writer.write_record(record)?;
    }
    writer.flush()?;
    Ok(())
}
