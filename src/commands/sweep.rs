use anyhow::{bail, Result};
use chrono::{Duration, NaiveDate, Utc};
use clap::{Args, ValueEnum};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use crate::analysis::{backtest, calibration, replay};
use crate::config;
use crate::db::Database;
use crate::strategy::registry;

#[derive(Debug, Clone, ValueEnum)]
pub enum RankBy {
    Roi,
    TotalPnl,
    Sharpe,
    WinRate,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum Preset {
    Quick,
    Full,
    Exhaustive,
}

#[derive(Debug, Clone, Args)]
pub struct SweepArgs {
    #[arg(long)]
    pub sweep_name: Option<String>,
    #[arg(long = "strategy")]
    pub strategies: Vec<String>,
    #[arg(long)]
    pub strategy_group: Option<String>,
    #[arg(long)]
    pub all_strategies: bool,
    #[arg(long)]
    pub backtest_ready_strategies: bool,
    #[arg(long, default_value = crate::DEFAULT_STRATEGIES_PATH)]
    pub strategies_file: PathBuf,
    #[arg(long, default_value = "sweep")]
    pub strategy_prefix: String,
    #[arg(long, default_value = "NYC")]
    pub city: String,
    #[arg(long)]
    pub nyc_all_available: bool,
    #[arg(long)]
    pub all_available: bool,
    #[arg(long, default_value_t = 60)]
    pub days: i64,
    #[arg(long)]
    pub date_from: Option<String>,
    #[arg(long)]
    pub date_to: Option<String>,
    #[arg(long, default_value = crate::DEFAULT_CALIBRATION_PATH)]
    pub calibration_file: PathBuf,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long, default_value = "backtest_param_finetune/results")]
    pub out_dir: PathBuf,
    #[arg(long, value_enum, default_value_t = RankBy::Roi)]
    pub rank_by: RankBy,
    #[arg(long, default_value_t = 5)]
    pub min_trades: i64,
    #[arg(long, value_enum, default_value_t = Preset::Quick)]
    pub preset: Preset,
    #[arg(long)]
    pub report: bool,
    #[arg(long, default_value_t = 1)]
    pub jobs: usize,
    #[arg(long)]
    pub min_ev_thresholds: Option<String>,
    #[arg(long)]
    pub desired_shares: Option<String>,
    #[arg(long)]
    pub funding_pcts: Option<String>,
    #[arg(long)]
    pub max_stake_usd: Option<String>,
    #[arg(long, default_value_t = 0.05)]
    pub fee_rate: f64,
    #[arg(long)]
    pub fixed_trade_stake_usd: Option<f64>,
    #[arg(long)]
    pub daily_max_stake_usd: Option<f64>,
    #[arg(long)]
    pub param_grid: Vec<String>,
    #[arg(long)]
    pub strategy_param_grid: Vec<String>,
}

pub fn run(mut args: SweepArgs) -> Result<()> {
    if args.nyc_all_available {
        args.city = "NYC".to_string();
        args.all_available = true;
        args.preset = Preset::Full;
    }
    let city = config::normalize_city(Some(&args.city)).unwrap_or(args.city.clone());
    let db_path = args
        .db
        .clone()
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    let db = Database::open(&db_path, true)?;
    let today = Utc::now().date_naive();
    let mut date_to = args
        .date_to
        .as_deref()
        .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
        .transpose()?
        .unwrap_or(today - Duration::days(1));
    let mut date_from = args
        .date_from
        .as_deref()
        .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
        .transpose()?
        .unwrap_or(date_to - Duration::days(args.days - 1));
    if args.all_available {
        let available = available_dates(&db, &city)?;
        if available.is_empty() {
            bail!("No backtest-ready dates found for {city}");
        }
        date_from = *available.first().unwrap();
        date_to = *available.last().unwrap();
    }
    let strategy_names = selected_strategies(&args)?;
    let calibration_table = calibration::load(&args.calibration_file)?;
    let dataset = Arc::new(replay::ReplayDataset::load(
        &db,
        date_from,
        date_to,
        Some(&city),
        backtest::DEFAULT_INTERVAL_MINUTES,
    )?);
    let min_evs = parse_float_list(args.min_ev_thresholds.as_deref())
        .unwrap_or_else(|| preset_values(&args.preset, "min_ev_threshold"));
    let desired_shares = parse_float_list(args.desired_shares.as_deref())
        .unwrap_or_else(|| preset_values(&args.preset, "desired_shares"));
    let daily_cap = parse_float_list(args.max_stake_usd.as_deref())
        .and_then(|values| values.first().copied())
        .or(args.daily_max_stake_usd)
        .unwrap_or(10.0);

    std::fs::create_dir_all(&args.out_dir)?;
    let sweep_name = args.sweep_name.clone().unwrap_or_else(|| {
        format!(
            "{}_{}",
            city.to_lowercase(),
            Utc::now().format("%Y%m%dT%H%M%SZ")
        )
    });
    let mut tasks = Vec::new();
    let mut combo_index = 0usize;
    for strategy in &strategy_names {
        for params in iter_param_grid(strategy, &args, &min_evs, &desired_shares, daily_cap)? {
            combo_index += 1;
            tasks.push((combo_index, strategy.clone(), params));
        }
    }
    let mut rows = run_sweep_tasks(
        tasks,
        &args,
        dataset,
        date_from,
        date_to,
        &city,
        &calibration_table,
    )?;
    rows.sort_by(|a, b| {
        b.rank_score
            .unwrap_or(f64::NEG_INFINITY)
            .partial_cmp(&a.rank_score.unwrap_or(f64::NEG_INFINITY))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.total_pnl
                    .partial_cmp(&a.total_pnl)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    let csv_path = args.out_dir.join(format!("{sweep_name}.csv"));
    let md_path = args.out_dir.join(format!("{sweep_name}.md"));
    let final_md_path = args
        .out_dir
        .join(format!("{sweep_name}__best_by_strategy.md"));
    write_sweep_csv(&csv_path, &rows)?;
    write_sweep_report(
        &md_path,
        &rows,
        &args.rank_by,
        &sweep_name,
        &city,
        date_from,
        date_to,
        &csv_path,
    )?;
    let strategy_artifacts =
        write_strategy_artifacts(&args.out_dir, &sweep_name, &rows, &args.rank_by)?;
    write_best_by_strategy_report(&final_md_path, &rows, &args.rank_by)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "csv": csv_path,
            "report": md_path,
            "best_by_strategy_report": final_md_path,
            "strategy_artifacts": strategy_artifacts,
            "rows": rows.len(),
            "top": rows.first()
        }))?
    );
    Ok(())
}

fn iter_param_grid(
    strategy_name: &str,
    args: &SweepArgs,
    min_evs: &[f64],
    desired_shares: &[f64],
    daily_cap: f64,
) -> Result<Vec<Value>> {
    let mut grid = BTreeMap::<String, Vec<f64>>::new();
    grid.insert("min_ev_threshold".to_string(), min_evs.to_vec());
    grid.insert("desired_shares".to_string(), desired_shares.to_vec());
    for (key, values) in preset_extra_grid(&args.preset, strategy_name) {
        grid.insert(key.to_string(), values);
    }
    for (key, values) in parse_param_grid(&args.param_grid)? {
        grid.insert(key, values);
    }
    for (target, key, values) in parse_strategy_param_grid(&args.strategy_param_grid)? {
        if target == strategy_name {
            grid.insert(key, values);
        }
    }
    if let Some(funding_values) = parse_float_list(args.funding_pcts.as_deref()) {
        grid.insert("funding_pct".to_string(), funding_values);
    } else {
        grid.remove("funding_pct");
    }

    let keys = grid.keys().cloned().collect::<Vec<_>>();
    let value_sets = keys
        .iter()
        .map(|key| grid.get(key).cloned().unwrap_or_else(|| vec![0.0]))
        .collect::<Vec<_>>();
    let combos = cartesian(&value_sets);
    let mut out = Vec::new();
    for values in combos {
        let mut params = registry::load_strategy_params(&args.strategies_file, strategy_name)?;
        if let Some(obj) = params.as_object_mut() {
            obj.remove("funding_pct");
            for (key, value) in keys.iter().zip(values.iter()) {
                obj.insert(key.clone(), json!(value));
            }
            obj.insert("strategy_id".to_string(), json!(strategy_name));
            obj.insert("max_stake_usd".to_string(), json!(daily_cap));
            obj.insert("fee_rate".to_string(), json!(args.fee_rate));
            obj.insert("sweep_name".to_string(), json!(args.sweep_name));
            if let Some(fixed) = args.fixed_trade_stake_usd {
                obj.insert("fixed_trade_stake_usd".to_string(), json!(fixed));
            }
        }
        out.push(params);
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
struct SweepRow {
    combo_index: usize,
    backtest_id: String,
    strategy: String,
    strategy_id: String,
    strategy_plugin: String,
    city: Option<String>,
    date_from: String,
    date_to: String,
    replay_mode: String,
    interval_minutes: i64,
    min_ev_threshold: Option<f64>,
    desired_shares: Option<f64>,
    max_stake_usd: Option<f64>,
    fixed_trade_stake_usd: Option<f64>,
    fee_rate: Option<f64>,
    params: Value,
    total_trades: usize,
    wins: usize,
    win_rate: f64,
    total_pnl: f64,
    total_stake: f64,
    sharpe: f64,
    roi: Option<f64>,
    rank_metric: String,
    rank_score: Option<f64>,
    diagnostics: String,
}

fn selected_strategies(args: &SweepArgs) -> Result<Vec<String>> {
    if args.all_strategies {
        return registry::all_members(&args.strategies_file);
    }
    if args.backtest_ready_strategies {
        return Ok(vec![
            "base_no_ev".into(),
            "base_yes_no_ev".into(),
            "ensemble_bucket_ev".into(),
            "dynamic_sigma_ev".into(),
            "bayesian_climatology_ev".into(),
            "corridor_ladder".into(),
            "threshold_union".into(),
            "basket_arb".into(),
            "maker_quote_sim".into(),
            "nowcast_metar".into(),
        ]);
    }
    if let Some(group) = args.strategy_group.as_deref() {
        return registry::group_members(&args.strategies_file, group);
    }
    if args.strategies.is_empty() {
        Ok(vec!["base_no_ev".into()])
    } else {
        Ok(args.strategies.clone())
    }
}

fn preset_values(preset: &Preset, key: &str) -> Vec<f64> {
    match (preset, key) {
        (Preset::Exhaustive, "min_ev_threshold") => {
            parse_float_list(Some("0.00:0.15:0.005")).unwrap()
        }
        (Preset::Exhaustive, "desired_shares") => parse_float_list(Some("1:25:1")).unwrap(),
        (Preset::Full, "min_ev_threshold") => parse_float_list(Some("0.01:0.10:0.01")).unwrap(),
        (Preset::Full, "desired_shares") => parse_float_list(Some("1:10:1")).unwrap(),
        (_, "min_ev_threshold") => parse_float_list(Some("0.02:0.08:0.02")).unwrap(),
        (_, "desired_shares") => parse_float_list(Some("1:10:1")).unwrap(),
        _ => vec![10.0],
    }
}

fn parse_float_list(raw: Option<&str>) -> Option<Vec<f64>> {
    let raw = raw?;
    let mut out = Vec::new();
    for item in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if item.contains(':') {
            let parts = item
                .split(':')
                .map(str::parse::<f64>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            if parts.len() != 3 || parts[2] == 0.0 {
                return None;
            }
            let (mut current, stop, step) = (parts[0], parts[1], parts[2]);
            let epsilon = step.abs() / 1_000_000.0;
            if step > 0.0 {
                while current <= stop + epsilon {
                    out.push((current * 10_000_000_000.0).round() / 10_000_000_000.0);
                    current += step;
                }
            } else {
                while current >= stop - epsilon {
                    out.push((current * 10_000_000_000.0).round() / 10_000_000_000.0);
                    current += step;
                }
            }
        } else {
            out.push(item.parse::<f64>().ok()?);
        }
    }
    let mut deduped = Vec::new();
    for value in out {
        if !deduped
            .iter()
            .any(|seen: &f64| (*seen - value).abs() < 1e-10)
        {
            deduped.push(value);
        }
    }
    Some(deduped)
}

fn parse_param_grid(raw: &[String]) -> Result<Vec<(String, Vec<f64>)>> {
    let mut out = Vec::new();
    for item in raw {
        let Some((key, values)) = item.split_once('=') else {
            bail!("expected KEY=v1,v2 for --param-grid");
        };
        out.push((
            key.to_string(),
            parse_float_list(Some(values)).unwrap_or_default(),
        ));
    }
    Ok(out)
}

fn parse_strategy_param_grid(raw: &[String]) -> Result<Vec<(String, String, Vec<f64>)>> {
    let mut out = Vec::new();
    for item in raw {
        let Some((strategy, rest)) = item.split_once(':') else {
            bail!("expected STRATEGY:KEY=v1,v2 for --strategy-param-grid");
        };
        let Some((key, values)) = rest.split_once('=') else {
            bail!("expected STRATEGY:KEY=v1,v2 for --strategy-param-grid");
        };
        out.push((
            strategy.to_string(),
            key.to_string(),
            parse_float_list(Some(values)).unwrap_or_default(),
        ));
    }
    Ok(out)
}

fn cartesian(value_sets: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let mut out = vec![Vec::new()];
    for values in value_sets {
        let mut next = Vec::new();
        for prefix in &out {
            for value in values {
                let mut row = prefix.clone();
                row.push(*value);
                next.push(row);
            }
        }
        out = next;
    }
    out
}

fn preset_extra_grid(preset: &Preset, strategy_name: &str) -> BTreeMap<&'static str, Vec<f64>> {
    let mut out = BTreeMap::new();
    match (preset, strategy_name) {
        (_, "ensemble_bucket_ev") => {
            out.insert("ensemble_sigma_floor", vec![0.8, 1.2, 1.6]);
        }
        (_, "dynamic_sigma_ev") => {
            out.insert("dynamic_sigma_min", vec![0.8, 1.0]);
            out.insert("spread_sigma_multiplier", vec![0.5]);
            if matches!(preset, Preset::Full) {
                out.insert("dynamic_sigma_max", vec![4.0, 5.0, 6.0]);
            }
        }
        (_, "bayesian_climatology_ev") => {
            out.insert("model_weight", vec![0.6, 0.8]);
        }
        (_, "corridor_ladder") => {
            out.insert("corridor_min_edge", vec![0.02, 0.05]);
            out.insert("max_corridor_width", vec![2.0, 3.0]);
        }
        (_, "basket_arb") => {
            out.insert("arb_buffer", vec![0.0, 0.005, 0.01]);
        }
        (_, "maker_quote_sim") => {
            out.insert("tick_size", vec![0.01, 0.02]);
            out.insert("maker_proxy_from_price_history", vec![1.0]);
        }
        _ => {}
    }
    if matches!(preset, Preset::Full | Preset::Exhaustive) {
        match strategy_name {
            "ensemble_bucket_ev" => {
                out.insert(
                    "ensemble_sigma_floor",
                    vec![0.6, 0.8, 1.0, 1.2, 1.4, 1.6, 2.0],
                );
            }
            "bayesian_climatology_ev" => {
                out.insert("model_weight", vec![0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]);
            }
            "basket_arb" => {
                out.insert("arb_buffer", vec![0.0, 0.0025, 0.005, 0.0075, 0.01, 0.02]);
            }
            "maker_quote_sim" => {
                out.insert("tick_size", vec![0.005, 0.01, 0.015, 0.02, 0.03]);
                out.insert("maker_proxy_from_price_history", vec![0.0, 1.0]);
            }
            _ => {}
        }
    }
    if matches!(preset, Preset::Exhaustive) {
        match strategy_name {
            "dynamic_sigma_ev" => {
                out.insert("dynamic_sigma_min", vec![0.5, 0.8, 1.0, 1.2]);
                out.insert("dynamic_sigma_max", vec![3.0, 4.0, 5.0, 6.0, 8.0]);
                out.insert("spread_sigma_multiplier", vec![0.25, 0.5, 0.75, 1.0]);
            }
            "corridor_ladder" | "threshold_union" => {
                out.insert("corridor_min_edge", vec![0.0, 0.01, 0.02, 0.05, 0.08]);
                out.insert("max_corridor_width", vec![1.0, 2.0, 3.0, 4.0]);
            }
            _ => {}
        }
    }
    out
}

fn run_sweep_tasks(
    tasks: Vec<(usize, String, Value)>,
    args: &SweepArgs,
    dataset: Arc<replay::ReplayDataset>,
    date_from: NaiveDate,
    date_to: NaiveDate,
    city: &str,
    calibration_table: &Value,
) -> Result<Vec<SweepRow>> {
    if args.jobs <= 1 || tasks.len() <= 1 {
        return tasks
            .into_iter()
            .map(|task| {
                run_one_sweep_task(
                    task,
                    args,
                    &dataset,
                    date_from,
                    date_to,
                    city,
                    calibration_table,
                )
            })
            .collect();
    }
    let jobs = args.jobs.min(tasks.len()).max(1);
    let chunk_size = (tasks.len() + jobs - 1) / jobs;
    let mut handles = Vec::new();
    for chunk in tasks.chunks(chunk_size) {
        let chunk = chunk.to_vec();
        let args = args.clone();
        let dataset = Arc::clone(&dataset);
        let city = city.to_string();
        let calibration_table = calibration_table.clone();
        handles.push(thread::spawn(
            move || -> std::result::Result<Vec<SweepRow>, String> {
                chunk
                    .into_iter()
                    .map(|task| {
                        run_one_sweep_task(
                            task,
                            &args,
                            &dataset,
                            date_from,
                            date_to,
                            &city,
                            &calibration_table,
                        )
                        .map_err(|e| e.to_string())
                    })
                    .collect()
            },
        ));
    }
    let mut rows = Vec::new();
    for handle in handles {
        rows.extend(
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("sweep worker panicked"))?
                .map_err(|e| anyhow::anyhow!(e))?,
        );
    }
    Ok(rows)
}

fn run_one_sweep_task(
    task: (usize, String, Value),
    args: &SweepArgs,
    dataset: &replay::ReplayDataset,
    date_from: NaiveDate,
    date_to: NaiveDate,
    city: &str,
    calibration_table: &Value,
) -> Result<SweepRow> {
    let (combo_index, strategy, params) = task;
    let summary = backtest::run_backtest_with_dataset(
        dataset,
        backtest::BacktestRequest {
            strategy_name: format!("{}_{}_{combo_index:04}", args.strategy_prefix, strategy),
            params: params.clone(),
            date_from,
            date_to,
            city: Some(city.to_string()),
            calibration_table: calibration_table.clone(),
            include_trades: false,
        },
    )?;
    let roi = if summary.total_stake > 0.0 {
        Some(summary.total_pnl / summary.total_stake)
    } else {
        None
    };
    Ok(SweepRow {
        combo_index,
        backtest_id: summary.backtest_id.clone(),
        strategy: summary.strategy.clone(),
        strategy_id: strategy,
        strategy_plugin: summary.strategy_plugin.clone(),
        city: summary.city.clone(),
        date_from: summary.date_from.clone(),
        date_to: summary.date_to.clone(),
        replay_mode: summary
            .params
            .get("replay_mode")
            .and_then(Value::as_str)
            .unwrap_or("interval")
            .to_string(),
        interval_minutes: summary.interval_minutes,
        min_ev_threshold: params.get("min_ev_threshold").and_then(Value::as_f64),
        desired_shares: params.get("desired_shares").and_then(Value::as_f64),
        max_stake_usd: params.get("max_stake_usd").and_then(Value::as_f64),
        fixed_trade_stake_usd: params.get("fixed_trade_stake_usd").and_then(Value::as_f64),
        fee_rate: params.get("fee_rate").and_then(Value::as_f64),
        params,
        total_trades: summary.total_trades,
        wins: summary.wins,
        win_rate: summary.win_rate,
        total_pnl: summary.total_pnl,
        total_stake: summary.total_stake,
        sharpe: summary.sharpe,
        roi,
        rank_metric: format!("{:?}", args.rank_by).to_lowercase(),
        rank_score: metric_from_values(
            summary.total_trades,
            summary.total_pnl,
            summary.total_stake,
            summary.sharpe,
            summary.win_rate,
            &args.rank_by,
            args.min_trades,
        ),
        diagnostics: serde_json::to_string(&summary.diagnostics)?,
    })
}

fn metric_from_values(
    total_trades: usize,
    total_pnl: f64,
    total_stake: f64,
    sharpe: f64,
    win_rate: f64,
    rank_by: &RankBy,
    min_trades: i64,
) -> Option<f64> {
    if total_trades < min_trades.max(0) as usize {
        return None;
    }
    match rank_by {
        RankBy::Roi => (total_stake > 0.0).then(|| total_pnl / total_stake),
        RankBy::TotalPnl => Some(total_pnl),
        RankBy::Sharpe => Some(sharpe),
        RankBy::WinRate => Some(win_rate),
    }
}

fn write_sweep_csv(path: &Path, rows: &[SweepRow]) -> Result<()> {
    let mut writer = csv::Writer::from_path(path)?;
    writer.write_record([
        "combo_index",
        "backtest_id",
        "strategy",
        "strategy_id",
        "strategy_plugin",
        "city",
        "date_from",
        "date_to",
        "replay_mode",
        "interval_minutes",
        "min_ev_threshold",
        "desired_shares",
        "max_stake_usd",
        "fixed_trade_stake_usd",
        "fee_rate",
        "total_trades",
        "wins",
        "win_rate",
        "total_pnl",
        "total_stake",
        "roi",
        "sharpe",
        "diagnostics",
        "rank_metric",
        "rank_score",
        "params_json",
        "param.desired_shares",
        "param.fee_rate",
        "param.fixed_trade_stake_usd",
        "param.interval_minutes",
        "param.max_stake_usd",
        "param.min_ev_threshold",
        "param.replay_mode",
        "param.strategy_id",
        "param.sweep_name",
        "combo_count",
    ])?;
    let combo_count = rows.len();
    for row in rows {
        writer.write_record([
            row.combo_index.to_string(),
            row.backtest_id.clone(),
            row.strategy.clone(),
            row.strategy_id.clone(),
            row.strategy_plugin.clone(),
            row.city.clone().unwrap_or_default(),
            row.date_from.clone(),
            row.date_to.clone(),
            row.replay_mode.clone(),
            row.interval_minutes.to_string(),
            opt_f64(row.min_ev_threshold),
            opt_f64(row.desired_shares),
            opt_f64(row.max_stake_usd),
            opt_f64(row.fixed_trade_stake_usd),
            opt_f64(row.fee_rate),
            row.total_trades.to_string(),
            row.wins.to_string(),
            row.win_rate.to_string(),
            row.total_pnl.to_string(),
            row.total_stake.to_string(),
            row.roi.map(|v| v.to_string()).unwrap_or_default(),
            row.sharpe.to_string(),
            row.diagnostics.clone(),
            row.rank_metric.clone(),
            row.rank_score.map(|v| v.to_string()).unwrap_or_default(),
            serde_json::to_string(&row.params)?,
            param_csv(&row.params, "desired_shares"),
            param_csv(&row.params, "fee_rate"),
            param_csv(&row.params, "fixed_trade_stake_usd"),
            param_csv(&row.params, "interval_minutes"),
            param_csv(&row.params, "max_stake_usd"),
            param_csv(&row.params, "min_ev_threshold"),
            param_csv(&row.params, "replay_mode"),
            param_csv(&row.params, "strategy_id"),
            param_csv(&row.params, "sweep_name"),
            combo_count.to_string(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn opt_f64(value: Option<f64>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

fn param_csv(params: &Value, key: &str) -> String {
    match params.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(v) => v.to_string(),
    }
}

fn write_sweep_report(
    path: &Path,
    rows: &[SweepRow],
    rank_by: &RankBy,
    sweep_name: &str,
    city: &str,
    date_from: NaiveDate,
    date_to: NaiveDate,
    csv_path: &Path,
) -> Result<()> {
    let mut lines = vec![
        format!("# Parameter Sweep Report: {sweep_name}"),
        String::new(),
        "## Scope".to_string(),
        String::new(),
        format!("- City: `{city}`"),
        format!("- Date range: `{date_from}` to `{date_to}`"),
        format!("- Parameter combinations: `{}`", rows.len()),
        format!("- CSV: `{}`", csv_path.display()),
        format!("- Ranked by: `{:?}`", rank_by),
        format!("- Rows: `{}`", rows.len()),
        String::new(),
        "## Top Results".to_string(),
        String::new(),
        "| Rank | Strategy | Trades | Win% | PnL | Sharpe | ROI |".to_string(),
        "|---:|---|---:|---:|---:|---:|---:|".to_string(),
    ];
    for (idx, row) in rows.iter().take(50).enumerate() {
        lines.push(format!(
            "| {} | `{}` | {} | {:.2}% | {:.4} | {:.4} | {:.2}% |",
            idx + 1,
            row.strategy,
            row.total_trades,
            row.win_rate * 100.0,
            row.total_pnl,
            row.sharpe,
            row.roi.unwrap_or(0.0) * 100.0
        ));
    }
    std::fs::write(path, lines.join("\n"))?;
    Ok(())
}

fn write_strategy_artifacts(
    out_dir: &Path,
    sweep_name: &str,
    rows: &[SweepRow],
    rank_by: &RankBy,
) -> Result<Map<String, Value>> {
    let mut grouped = BTreeMap::<String, Vec<SweepRow>>::new();
    for row in rows {
        grouped
            .entry(row.strategy_id.clone())
            .or_default()
            .push(row.clone());
    }
    let mut artifacts = Map::new();
    for (strategy, mut strategy_rows) in grouped {
        strategy_rows.sort_by(|a, b| {
            b.rank_score
                .unwrap_or(f64::NEG_INFINITY)
                .partial_cmp(&a.rank_score.unwrap_or(f64::NEG_INFINITY))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let csv = out_dir.join(format!("{sweep_name}__{strategy}.csv"));
        let md = out_dir.join(format!("{sweep_name}__{strategy}.md"));
        write_sweep_csv(&csv, &strategy_rows)?;
        write_sweep_report(
            &md,
            &strategy_rows,
            rank_by,
            &format!("{sweep_name} / {strategy}"),
            strategy_rows
                .first()
                .and_then(|row| row.city.as_deref())
                .unwrap_or(""),
            NaiveDate::parse_from_str(
                strategy_rows
                    .first()
                    .map(|row| row.date_from.as_str())
                    .unwrap_or("1970-01-01"),
                "%Y-%m-%d",
            )?,
            NaiveDate::parse_from_str(
                strategy_rows
                    .first()
                    .map(|row| row.date_to.as_str())
                    .unwrap_or("1970-01-01"),
                "%Y-%m-%d",
            )?,
            &csv,
        )?;
        artifacts.insert(strategy, json!({"csv": csv, "report": md}));
    }
    Ok(artifacts)
}

fn write_best_by_strategy_report(path: &Path, rows: &[SweepRow], rank_by: &RankBy) -> Result<()> {
    let mut best = BTreeMap::<String, &SweepRow>::new();
    for row in rows {
        let replace = best
            .get(&row.strategy_id)
            .map(|current| {
                row.rank_score.unwrap_or(f64::NEG_INFINITY)
                    > current.rank_score.unwrap_or(f64::NEG_INFINITY)
            })
            .unwrap_or(true);
        if replace {
            best.insert(row.strategy_id.clone(), row);
        }
    }
    let mut rows = best.into_values().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.rank_score
            .unwrap_or(f64::NEG_INFINITY)
            .partial_cmp(&a.rank_score.unwrap_or(f64::NEG_INFINITY))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut lines = vec![
        "# Final Strategy Comparison".to_string(),
        String::new(),
        format!("- Ranked by: `{:?}`", rank_by),
        String::new(),
        "| Rank | Strategy | ROI | PnL | Stake | Trades | Win% | Sharpe | Rank Score |".to_string(),
        "|---:|---|---:|---:|---:|---:|---:|---:|---:|".to_string(),
    ];
    for (idx, row) in rows.iter().enumerate() {
        lines.push(format!(
            "| {} | `{}` | {:.2}% | {:.4} | {:.4} | {} | {:.2}% | {:.4} | {} |",
            idx + 1,
            row.strategy_id,
            row.roi.unwrap_or(0.0) * 100.0,
            row.total_pnl,
            row.total_stake,
            row.total_trades,
            row.win_rate * 100.0,
            row.sharpe,
            row.rank_score
                .map(|v| format!("{v:.6}"))
                .unwrap_or_default(),
        ));
    }
    std::fs::write(path, lines.join("\n"))?;
    Ok(())
}

fn available_dates(db: &Database, city: &str) -> Result<Vec<NaiveDate>> {
    let mut stmt = db.conn().prepare(
        "SELECT target_date
         FROM settlement_records
         WHERE city = ?1 AND winning_bin IS NOT NULL
         ORDER BY target_date",
    )?;
    let mut out = Vec::new();
    for row in stmt.query_map([city], |row| row.get::<_, String>(0))? {
        let target_date = row?;
        if has_usable_forecast(db, city, &target_date)?
            && has_usable_price_bins(db, city, &target_date)?
        {
            out.push(NaiveDate::parse_from_str(&target_date, "%Y-%m-%d")?);
        }
    }
    Ok(out)
}

fn has_usable_forecast(db: &Database, city: &str, target_date: &str) -> Result<bool> {
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*)
         FROM forecast_records
         WHERE city = ?1 AND target_date = ?2
           AND source IN ('open-meteo', 'gfs-global-24h', 'historical-open-meteo', 'open-meteo-historical')
           AND (temp_max_gfs_f IS NOT NULL OR temp_max_ecmwf_f IS NOT NULL)",
        (city, target_date),
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn has_usable_price_bins(db: &Database, city: &str, target_date: &str) -> Result<bool> {
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(DISTINCT bin_label)
         FROM price_history_1m
         WHERE city = ?1 AND target_date = ?2
           AND hours_to_close > 0 AND hours_to_close <= 36",
        (city, target_date),
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[cfg(test)]
mod all_available_tests {
    use super::*;

    #[test]
    fn all_available_requires_settlement_forecast_and_prices() {
        let db = Database::open(":memory:", false).unwrap();
        db.conn()
            .execute(
                "INSERT INTO settlement_records (
                city, target_date, event_id, event_slug, winning_bin, settlement_temp_f,
                wunderground_station, raw_outcome_prices
            ) VALUES ('NYC','2026-04-01','e','s','70-71°F',70.5,'KLGA','{}')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO forecast_records (
                timestamp, city, target_date, source, temp_max_gfs_f, forecast_lead_hours
            ) VALUES ('2026-03-31T06:00:00Z','NYC','2026-04-01','gfs-global-24h',70.0,26.0)",
                [],
            )
            .unwrap();
        assert!(available_dates(&db, "NYC").unwrap().is_empty());
        db.conn().execute(
            "INSERT INTO price_history_1m (
                city, target_date, bin_label, bin_index, timestamp, yes_price, hours_to_close, source
            ) VALUES ('NYC','2026-04-01','70-71°F',0,'2026-03-31T12:00:00Z',0.5,24.0,'test')",
            [],
        ).unwrap();
        assert_eq!(
            available_dates(&db, "NYC").unwrap(),
            vec![NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()]
        );
    }
}
