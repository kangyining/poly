use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::analysis::calibration;
use crate::config;
use crate::db;
use crate::strategy::*;

pub const TRADING_WINDOW_HOURS: f64 = 36.0;
pub const DEFAULT_INTERVAL_MINUTES: i64 = 1;

#[derive(Debug, Clone)]
pub struct BacktestRequest {
    pub strategy_name: String,
    pub params: Value,
    pub date_from: NaiveDate,
    pub date_to: NaiveDate,
    pub city: Option<String>,
    pub calibration_table: Value,
    pub include_trades: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestTrade {
    pub city: String,
    pub target_date: String,
    pub bin_label: String,
    pub winning_bin: String,
    pub forecast_source: String,
    pub forecast_f: f64,
    pub forecast_lead_hours: f64,
    pub beta_f: f64,
    pub mu_f: f64,
    pub sigma_f: f64,
    pub p_model: Option<f64>,
    pub effective_ask: f64,
    pub fill_source: String,
    pub fee_rate: f64,
    pub ev_no: f64,
    pub hours_to_close: f64,
    pub shares: f64,
    pub stake_usd: f64,
    pub pnl: f64,
    pub won: bool,
    pub opened_at: Option<String>,
    pub settled_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestSummary {
    pub backtest_id: String,
    pub strategy: String,
    pub params: Value,
    pub city: Option<String>,
    pub date_from: String,
    pub date_to: String,
    pub total_trades: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub total_pnl: f64,
    pub sharpe: f64,
    pub total_stake: f64,
    pub turnover_yield: Option<f64>,
    pub pnl_per_calendar_day: Option<f64>,
    pub pnl_per_active_day: Option<f64>,
    pub pnl_per_trade: Option<f64>,
    pub active_trade_days: usize,
    pub return_on_daily_cap: Option<f64>,
    pub result_storage: String,
    pub days_considered: usize,
    pub days_with_forecast: usize,
    pub days_with_price_history: usize,
    pub days_with_minute_price_history: usize,
    pub days_with_snapshot_fills: usize,
    pub days_missing_forecast: usize,
    pub days_missing_price_history: usize,
    pub interval_minutes: i64,
    pub interval_bars_considered: usize,
    pub intervals_with_trades: usize,
    pub forecast_switches: usize,
    pub strategy_plugin: String,
    pub diagnostics: BTreeMap<String, usize>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub trades: Vec<BacktestTradeRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestTradeRow {
    pub backtest_id: String,
    pub strategy_name: String,
    pub target_date: String,
    pub event_slug: String,
    pub bin_label: String,
    pub entry_price: f64,
    pub fee_rate: f64,
    pub shares: f64,
    pub stake_usd: f64,
    pub pnl: f64,
    pub status: String,
    pub opened_at: String,
    pub settled_at: String,
    pub token_id: String,
    pub market_id: String,
    pub trade_rationale: String,
}

pub fn run_backtest(db: &db::Database, req: BacktestRequest) -> Result<BacktestSummary> {
    let backtest_id = uuid::Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let mut params = req.params.clone();
    if let Some(city) = &req.city {
        params["city"] = Value::String(city.clone());
    }
    params["replay_mode"] = Value::String("interval".to_string());
    params["interval_minutes"] = json!(DEFAULT_INTERVAL_MINUTES);

    let strategy_plugin =
        crate::strategy::registry::resolve_strategy_name(&req.strategy_name, &params);
    let strategy = strategy_by_name(&strategy_plugin);
    let settlements = db::load_settlements(
        db,
        &req.date_from.to_string(),
        &req.date_to.to_string(),
        req.city.as_deref(),
    )?;

    let max_stake = params
        .get("max_stake_usd")
        .and_then(Value::as_f64)
        .unwrap_or(50.0);
    let max_total_stake = params.get("max_total_stake_usd").and_then(Value::as_f64);
    let fee_rate = params
        .get("fee_rate")
        .and_then(Value::as_f64)
        .unwrap_or(0.05);
    let desired_shares = params
        .get("desired_shares")
        .and_then(Value::as_f64)
        .unwrap_or(10.0);
    let funding_pct = params
        .get("funding_pct")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let shares = desired_shares * funding_pct;

    let mut trades = Vec::<BacktestTrade>::new();
    let mut trade_rows = Vec::<BacktestTradeRow>::new();
    let mut total_stake = 0.0;
    let mut days_considered = 0;
    let mut days_with_forecast = 0;
    let mut days_with_price_history = 0;
    let mut days_with_minute_price_history = 0;
    let mut days_with_snapshot_fills = 0;
    let mut days_missing_forecast = 0;
    let mut days_missing_price_history = 0;
    let mut interval_bars_considered = 0;
    let mut intervals_with_trades = 0;
    let mut forecast_switches = 0;
    let mut diagnostics = Vec::<StrategyDiagnostic>::new();

    for settlement in settlements {
        let settlement_city = settlement.city;
        let target_date = settlement.target_date;
        let Some(winning_bin) = settlement.winning_bin else {
            continue;
        };
        if winning_bin.is_empty() {
            continue;
        }
        days_considered += 1;
        let event_slug = settlement
            .event_slug
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| config::default_event_slug(&settlement_city, &target_date));
        let market_open = market_open_utc(&target_date)?;
        let forecast_snapshots =
            db::load_forecast_snapshots(db, &settlement_city, &target_date, market_open)?;
        if forecast_snapshots.is_empty() {
            days_missing_forecast += 1;
            continue;
        }
        days_with_forecast += 1;
        let price_rows =
            db::load_price_bars(db, &settlement_city, &target_date, TRADING_WINDOW_HOURS)?;
        if price_rows.is_empty() {
            days_missing_price_history += 1;
            continue;
        }
        days_with_price_history += 1;
        days_with_minute_price_history += 1;
        let price_bars = group_price_bars(price_rows, DEFAULT_INTERVAL_MINUTES);
        let snapshot_fills = db::load_snapshot_fill_bars(
            db,
            &settlement_city,
            &target_date,
            DEFAULT_INTERVAL_MINUTES,
            shares,
            TRADING_WINDOW_HOURS,
        )
        .unwrap_or_default();
        let station = config::station_for_city(&settlement_city);
        let target_dt = NaiveDate::parse_from_str(&target_date, "%Y-%m-%d")?;
        let metar_records =
            db::load_metar_records(db, &settlement_city, &target_date).unwrap_or_default();
        let climatology =
            db::load_climatology_temps(db, &settlement_city, &target_date, 14).unwrap_or_default();
        let mut day_stake = 0.0;
        let mut opened_bins = BTreeSet::<String>::new();
        let mut last_forecast_key: Option<String> = None;
        let mut used_snapshot_fill = false;

        for (bucket_ts, bucket) in price_bars {
            if bucket.is_empty() {
                continue;
            }
            interval_bars_considered += 1;
            let current_lead_hours = bucket
                .reference_market()
                .map(|bar| bar.hours_to_close)
                .unwrap_or(0.0);
            let Some(active_forecast) =
                select_active_forecast(&forecast_snapshots, bucket_ts, current_lead_hours)
            else {
                continue;
            };
            let forecast_key = format!(
                "{}:{}:{:.4}",
                active_forecast.source, active_forecast.available_at, active_forecast.forecast_f
            );
            if Some(forecast_key.clone()) != last_forecast_key {
                forecast_switches += 1;
                last_forecast_key = Some(forecast_key);
            }
            let (beta_f, sigma_f) = calibration::lookup(
                &req.calibration_table,
                &station,
                target_dt.month(),
                current_lead_hours,
            );
            let forecast = ForecastState {
                source: active_forecast.source.clone(),
                forecast_f: active_forecast.forecast_f,
                lead_hours: current_lead_hours,
                beta_f,
                sigma_f,
                mu_f: active_forecast.forecast_f + beta_f,
                gfs_f: active_forecast.gfs_f,
                ecmwf_f: active_forecast.ecmwf_f,
                model_spread_f: active_forecast.model_spread_c.map(|c| c * 9.0 / 5.0),
            };
            let mut market_bins = Vec::new();
            let mut markets = bucket.bars.values().collect::<Vec<_>>();
            markets.sort_by_key(|market| market.bin_index);
            for market in markets {
                if opened_bins.contains(&format!("{}:{}", strategy.name(), market.bin_label)) {
                    continue;
                }
                let snapshot_fill = snapshot_fills.get(&(bucket_ts, market.bin_label.clone()));
                if snapshot_fill.is_some() {
                    used_snapshot_fill = true;
                }
                market_bins.push(build_market_bin(
                    market,
                    snapshot_fill,
                    fee_rate,
                    current_lead_hours,
                    bucket_ts,
                ));
            }
            let context = StrategyContext {
                city: settlement_city.clone(),
                target_date: target_date.clone(),
                winning_bin: winning_bin.clone(),
                bins: market_bins,
                forecast: Some(forecast.clone()),
                params: params.clone(),
                replay_mode: "interval".to_string(),
                evaluation_time: Some(bucket_ts),
                event_slug: Some(event_slug.clone()),
                settlement_temp_f: settlement.settlement_temp_f,
                metar_max_so_far_f: metar_max_before(&metar_records, bucket_ts),
                climatology_temps_f: climatology.clone(),
            };
            let output = strategy.generate_candidates(&context);
            diagnostics.extend(output.diagnostics);

            let mut trades_this_interval = 0;
            for mut candidate in output.candidates {
                let opened_key = format!(
                    "{}:{}:{}",
                    candidate.strategy_name, candidate.side, candidate.bin_label
                );
                if opened_bins.contains(&opened_key) {
                    continue;
                }
                if day_stake + candidate.stake_usd > max_stake {
                    continue;
                }
                if let Some(cap) = max_total_stake {
                    if total_stake + candidate.stake_usd > cap {
                        continue;
                    }
                }
                let settled_at = market_close_utc(&target_date)?.to_rfc3339();
                candidate.settled_at = Some(settled_at.clone());
                let (pnl, won) = candidate_pnl(&candidate, &winning_bin);
                day_stake += candidate.stake_usd;
                total_stake += candidate.stake_usd;
                opened_bins.insert(opened_key);
                let trade = BacktestTrade {
                    city: settlement_city.clone(),
                    target_date: target_date.clone(),
                    bin_label: candidate.bin_label.clone(),
                    winning_bin: winning_bin.clone(),
                    forecast_source: active_forecast.source.clone(),
                    forecast_f: active_forecast.forecast_f,
                    forecast_lead_hours: current_lead_hours,
                    beta_f,
                    mu_f: active_forecast.forecast_f + beta_f,
                    sigma_f,
                    p_model: candidate.p_model,
                    effective_ask: candidate.effective_ask,
                    fill_source: candidate.fill_source.clone(),
                    fee_rate: candidate.fee_rate,
                    ev_no: candidate.edge,
                    hours_to_close: current_lead_hours,
                    shares: candidate.shares,
                    stake_usd: candidate.stake_usd,
                    pnl,
                    won,
                    opened_at: candidate.opened_at.clone(),
                    settled_at: Some(settled_at.clone()),
                };
                if req.include_trades {
                    trade_rows.push(backtest_trade_row(
                        &backtest_id,
                        &req.strategy_name,
                        &created_at,
                        &settlement_city,
                        &target_date,
                        &event_slug,
                        &candidate,
                        pnl,
                    ));
                }
                trades.push(trade);
                trades_this_interval += 1;
            }
            if trades_this_interval > 0 {
                intervals_with_trades += 1;
            }
        }
        if used_snapshot_fill {
            days_with_snapshot_fills += 1;
        }
    }

    let mut summary = finalize(
        backtest_id,
        req.strategy_name,
        params,
        req.city,
        req.date_from,
        req.date_to,
        trades,
        total_stake,
        days_considered,
    );
    summary.days_with_forecast = days_with_forecast;
    summary.days_with_price_history = days_with_price_history;
    summary.days_with_minute_price_history = days_with_minute_price_history;
    summary.days_with_snapshot_fills = days_with_snapshot_fills;
    summary.days_missing_forecast = days_missing_forecast;
    summary.days_missing_price_history = days_missing_price_history;
    summary.interval_minutes = DEFAULT_INTERVAL_MINUTES;
    summary.interval_bars_considered = interval_bars_considered;
    summary.intervals_with_trades = intervals_with_trades;
    summary.forecast_switches = forecast_switches;
    summary.strategy_plugin = strategy_plugin;
    summary.diagnostics = diagnostic_counts(&diagnostics);
    summary.trades = trade_rows;
    Ok(summary)
}

#[derive(Debug, Clone, Default)]
struct PriceBarBucket {
    order: Vec<String>,
    bars: BTreeMap<String, db::PriceBar>,
}

impl PriceBarBucket {
    fn insert(&mut self, row: db::PriceBar) {
        if !self.bars.contains_key(&row.bin_label) {
            self.order.push(row.bin_label.clone());
        }
        self.bars.insert(row.bin_label.clone(), row);
    }

    fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    fn reference_market(&self) -> Option<&db::PriceBar> {
        self.order.first().and_then(|label| self.bars.get(label))
    }
}

fn group_price_bars(
    rows: Vec<db::PriceBar>,
    interval_minutes: i64,
) -> BTreeMap<DateTime<Utc>, PriceBarBucket> {
    let mut out: BTreeMap<DateTime<Utc>, PriceBarBucket> = BTreeMap::new();
    for row in rows {
        let bucket = floor_timestamp(row.timestamp, interval_minutes);
        out.entry(bucket).or_default().insert(row);
    }
    out
}

fn floor_timestamp(ts: DateTime<Utc>, interval_minutes: i64) -> DateTime<Utc> {
    let minute = (ts.minute() as i64 / interval_minutes) * interval_minutes;
    ts.with_minute(minute as u32)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap()
}

fn select_active_forecast(
    snapshots: &[db::ForecastSnapshot],
    evaluation_time: DateTime<Utc>,
    target_lead_hours: f64,
) -> Option<db::ForecastSnapshot> {
    let mut candidates = snapshots
        .iter()
        .filter(|snapshot| snapshot.available_at <= evaluation_time)
        .cloned()
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        candidates = snapshots.to_vec();
    }
    candidates.sort_by(|a, b| {
        let a_lead = a.forecast_lead_hours.unwrap_or(target_lead_hours);
        let b_lead = b.forecast_lead_hours.unwrap_or(target_lead_hours);
        a.priority
            .cmp(&b.priority)
            .then_with(|| {
                (a_lead - target_lead_hours)
                    .abs()
                    .partial_cmp(&(b_lead - target_lead_hours).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| b.available_at.cmp(&a.available_at))
    });
    candidates.into_iter().next()
}

fn build_market_bin(
    market: &db::PriceBar,
    snapshot_fill: Option<&db::SnapshotFill>,
    fee_rate: f64,
    fallback_hours_to_close: f64,
    opened_at: DateTime<Utc>,
) -> MarketBin {
    let yes_price = market.yes_price.clamp(0.0, 1.0);
    let (
        effective_ask_no,
        effective_fee_rate,
        hours_to_close,
        no_best_bid,
        no_best_ask,
        fill_source,
    ) = if let Some(fill) = snapshot_fill {
        (
            fill.effective_ask.clamp(0.0, 1.0),
            fill.fee_rate.unwrap_or(fee_rate),
            fill.hours_to_close,
            fill.no_best_bid,
            fill.no_best_ask,
            "market_snapshots".to_string(),
        )
    } else {
        (
            (1.0 - yes_price).clamp(0.0, 1.0),
            fee_rate,
            fallback_hours_to_close,
            None,
            None,
            "price_history_proxy".to_string(),
        )
    };
    MarketBin {
        bin_label: market.bin_label.clone(),
        bin_index: market.bin_index,
        yes_price,
        no_price: (1.0 - yes_price).clamp(0.0, 1.0),
        effective_ask_no,
        fee_rate: effective_fee_rate,
        hours_to_close,
        token_id_yes: market.token_id_yes.clone(),
        token_id_no: None,
        no_best_bid,
        no_best_ask,
        fill_source,
        opened_at: Some(opened_at.to_rfc3339()),
    }
}

pub fn market_close_utc(target_date: &str) -> Result<DateTime<Utc>> {
    let target = NaiveDate::parse_from_str(target_date, "%Y-%m-%d")?;
    Ok(target.and_hms_opt(12, 0, 0).unwrap().and_utc())
}

pub fn market_open_utc(target_date: &str) -> Result<DateTime<Utc>> {
    Ok(market_close_utc(target_date)? - Duration::hours(TRADING_WINDOW_HOURS as i64))
}

fn metar_max_before(
    records: &[(DateTime<Utc>, f64)],
    evaluation_time: DateTime<Utc>,
) -> Option<f64> {
    records
        .iter()
        .filter(|(ts, _)| *ts <= evaluation_time)
        .map(|(_, temp)| *temp)
        .reduce(f64::max)
}

pub fn candidate_pnl(candidate: &StrategyCandidate, winning_bin: &str) -> (f64, bool) {
    let cost = entry_cost(candidate.effective_ask, candidate.fee_rate);
    match candidate.side.as_str() {
        "YES" | "NO" => settle_single(
            &candidate.side,
            &candidate.bin_label,
            winning_bin,
            candidate.shares,
            cost,
        ),
        "YES_CORRIDOR" | "YES_LOW_TAIL" | "YES_HIGH_TAIL" | "YES_BASKET" => {
            let bins = candidate
                .metadata
                .get("underlying_bins")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v.as_str().map(ToString::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            settle_union(
                winning_bin,
                &bins,
                candidate.shares,
                candidate.effective_ask,
            )
        }
        _ => settle_single(
            &candidate.side,
            &candidate.bin_label,
            winning_bin,
            candidate.shares,
            cost,
        ),
    }
}

fn backtest_trade_row(
    backtest_id: &str,
    strategy_name: &str,
    created_at: &str,
    city: &str,
    target_date: &str,
    event_slug: &str,
    candidate: &StrategyCandidate,
    pnl: f64,
) -> BacktestTradeRow {
    let opened_at = candidate
        .opened_at
        .clone()
        .unwrap_or_else(|| created_at.to_string());
    let settled_at = candidate
        .settled_at
        .clone()
        .unwrap_or_else(|| created_at.to_string());
    let trade_rationale = {
        let mut rationale = candidate.metadata.clone();
        rationale.insert("strategy".to_string(), json!(candidate.strategy_name));
        rationale.insert("side".to_string(), json!(candidate.side));
        rationale.insert("p_model".to_string(), json!(candidate.p_model));
        rationale.insert("edge".to_string(), json!(candidate.edge));
        rationale.insert("effective_ask".to_string(), json!(candidate.effective_ask));
        rationale.insert("fee_rate".to_string(), json!(candidate.fee_rate));
        rationale.insert("fill_source".to_string(), json!(candidate.fill_source));
        Value::Object(rationale).to_string()
    };
    BacktestTradeRow {
        backtest_id: backtest_id.to_string(),
        strategy_name: strategy_name.to_string(),
        target_date: target_date.to_string(),
        event_slug: event_slug.to_string(),
        bin_label: candidate.bin_label.clone(),
        entry_price: candidate.effective_ask,
        fee_rate: candidate.fee_rate,
        shares: candidate.shares,
        stake_usd: candidate.stake_usd,
        pnl: round4_preserve_signed_zero(pnl),
        status: "closed".to_string(),
        opened_at,
        settled_at,
        token_id: candidate.token_id.clone(),
        market_id: if candidate.market_id.is_empty() {
            format!("backtest-{}-{}", city, target_date)
        } else {
            candidate.market_id.clone()
        },
        trade_rationale,
    }
}

fn finalize(
    backtest_id: String,
    strategy: String,
    params: Value,
    city: Option<String>,
    date_from: NaiveDate,
    date_to: NaiveDate,
    trades: Vec<BacktestTrade>,
    total_stake: f64,
    days_considered: usize,
) -> BacktestSummary {
    let total_pnl = trades.iter().map(|t| t.pnl).sum::<f64>();
    let wins = trades.iter().filter(|t| t.won).count();
    let win_rate = if trades.is_empty() {
        0.0
    } else {
        wins as f64 / trades.len() as f64
    };
    let trade_days = trades
        .iter()
        .map(|t| t.target_date.clone())
        .collect::<BTreeSet<_>>();
    let active_trade_days = trade_days.len();
    let mut daily = HashMap::<String, f64>::new();
    for trade in &trades {
        *daily.entry(trade.target_date.clone()).or_default() += trade.pnl;
    }
    let daily_pnls = daily.values().copied().collect::<Vec<_>>();
    let sharpe = if daily_pnls.len() > 1 {
        let mean = daily_pnls.iter().sum::<f64>() / daily_pnls.len() as f64;
        let variance = daily_pnls.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
            / (daily_pnls.len() - 1) as f64;
        let std = variance.sqrt();
        if std > 0.0 {
            mean / std * 252.0_f64.sqrt()
        } else {
            0.0
        }
    } else {
        0.0
    };
    let daily_cap_return = params
        .get("max_stake_usd")
        .and_then(Value::as_f64)
        .filter(|v| *v > 0.0)
        .map(|cap| total_pnl / cap);
    BacktestSummary {
        backtest_id,
        strategy,
        params,
        city,
        date_from: date_from.to_string(),
        date_to: date_to.to_string(),
        total_trades: trades.len(),
        wins,
        win_rate: round4(win_rate),
        total_pnl: round4(total_pnl),
        sharpe: round4(sharpe),
        total_stake: round4(total_stake),
        turnover_yield: (total_stake > 0.0).then(|| round6(total_pnl / total_stake)),
        pnl_per_calendar_day: (days_considered > 0)
            .then(|| round4(total_pnl / days_considered as f64)),
        pnl_per_active_day: (active_trade_days > 0)
            .then(|| round4(total_pnl / active_trade_days as f64)),
        pnl_per_trade: (!trades.is_empty()).then(|| round4(total_pnl / trades.len() as f64)),
        active_trade_days,
        return_on_daily_cap: daily_cap_return.map(round6),
        result_storage: "files".to_string(),
        days_considered,
        days_with_forecast: 0,
        days_with_price_history: 0,
        days_with_minute_price_history: 0,
        days_with_snapshot_fills: 0,
        days_missing_forecast: 0,
        days_missing_price_history: 0,
        interval_minutes: DEFAULT_INTERVAL_MINUTES,
        interval_bars_considered: 0,
        intervals_with_trades: 0,
        forecast_switches: 0,
        strategy_plugin: String::new(),
        diagnostics: BTreeMap::new(),
        trades: Vec::new(),
    }
}

fn diagnostic_counts(diagnostics: &[StrategyDiagnostic]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for diag in diagnostics {
        let key = format!("{}:{}", diag.strategy_name, diag.severity);
        *out.entry(key).or_default() += 1;
    }
    out
}

pub fn auto_artifact_paths(
    strategy_names: &[String],
    city: Option<&str>,
    date_from: NaiveDate,
    date_to: NaiveDate,
) -> BTreeMap<&'static str, PathBuf> {
    let city_slug = city.unwrap_or("all").to_lowercase().replace(' ', "-");
    let strategy_slug = if strategy_names.len() == 1 {
        strategy_names[0].clone()
    } else {
        "strategy_group".to_string()
    };
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let stem = format!(
        "{}_{}_{}_{}_{}",
        city_slug, date_from, date_to, strategy_slug, stamp
    );
    BTreeMap::from([
        (
            "summary_csv",
            PathBuf::from(format!("backtest/{}_summary.csv", stem)),
        ),
        (
            "trades_csv",
            PathBuf::from(format!("backtest/{}_trades.csv", stem)),
        ),
        (
            "report",
            PathBuf::from(format!("backtest/{}_report.md", stem)),
        ),
    ])
}

pub fn write_summary_csv(path: &Path, summaries: &[BacktestSummary]) -> Result<()> {
    ensure_parent(path)?;
    let rows = summaries
        .iter()
        .map(summary_to_flat_row)
        .collect::<Vec<_>>();
    write_flat_rows(
        path,
        &rows,
        &[
            "backtest_id",
            "strategy",
            "strategy_plugin",
            "city",
            "date_from",
            "date_to",
            "total_trades",
            "wins",
            "win_rate",
            "total_pnl",
            "sharpe",
            "total_stake",
            "turnover_yield",
            "pnl_per_calendar_day",
            "pnl_per_active_day",
            "pnl_per_trade",
            "active_trade_days",
            "return_on_daily_cap",
            "result_storage",
            "days_considered",
            "days_with_forecast",
            "days_with_price_history",
            "days_with_minute_price_history",
            "days_with_snapshot_fills",
            "days_missing_forecast",
            "days_missing_price_history",
            "interval_minutes",
            "interval_bars_considered",
            "intervals_with_trades",
            "forecast_switches",
            "params.strategy_id",
            "params.min_ev_threshold",
            "params.desired_shares",
            "params.funding_pct",
            "params.max_stake_usd",
            "params.fee_rate",
            "params.replay_mode",
        ],
    )
}

pub fn write_trades_csv(path: &Path, rows: &[BacktestTradeRow]) -> Result<()> {
    ensure_parent(path)?;
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_path(path)?;
    writer.write_record([
        "backtest_id",
        "strategy_name",
        "target_date",
        "event_slug",
        "bin_label",
        "entry_price",
        "fee_rate",
        "shares",
        "stake_usd",
        "pnl",
        "status",
        "opened_at",
        "settled_at",
        "token_id",
        "market_id",
        "trade_rationale",
    ])?;
    let mut sorted = rows.to_vec();
    sorted.sort_by_key(|r| {
        (
            r.strategy_name.clone(),
            r.target_date.clone(),
            r.opened_at.clone(),
            r.bin_label.clone(),
        )
    });
    for row in sorted {
        writer.serialize(row)?;
    }
    writer.flush()?;
    Ok(())
}

pub fn write_summary_report(
    path: &Path,
    summaries: &[BacktestSummary],
    summary_csv: &Path,
    trades_csv: &Path,
) -> Result<()> {
    ensure_parent(path)?;
    let total_trades = summaries.iter().map(|s| s.total_trades).sum::<usize>();
    let total_pnl = summaries.iter().map(|s| s.total_pnl).sum::<f64>();
    let first = summaries.first();
    let mut ranked = summaries.to_vec();
    ranked.sort_by(|a, b| {
        b.total_pnl
            .partial_cmp(&a.total_pnl)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut lines = Vec::new();
    lines.push(format!(
        "# Strategy Backtest Report: {}",
        first
            .and_then(|s| s.city.as_deref())
            .unwrap_or("All Cities")
    ));
    lines.push(String::new());
    lines.push("## Scope".to_string());
    lines.push(String::new());
    lines.push(format!(
        "- Date range: `{}` to `{}`",
        first.map(|s| s.date_from.as_str()).unwrap_or(""),
        first.map(|s| s.date_to.as_str()).unwrap_or("")
    ));
    lines.push("- Replay: `1-minute interval`".to_string());
    lines.push(format!("- Strategies run: `{}`", summaries.len()));
    lines.push(format!("- Total simulated trades: `{}`", total_trades));
    lines.push(format!("- Combined PnL: `{:.4}`", total_pnl));
    lines.push(format!("- Summary CSV: `{}`", summary_csv.display()));
    lines.push(format!("- Trades CSV: `{}`", trades_csv.display()));
    lines.push(String::new());
    lines.push("## Strategy Comparison".to_string());
    lines.push(String::new());
    lines.push("| Rank | Strategy | Plugin | Trades | Win% | PnL | Stake Turnover | Turnover Yield | Sharpe | Days | Diagnostics |".to_string());
    lines.push("|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---|".to_string());
    for (idx, summary) in ranked.iter().enumerate() {
        lines.push(format!(
            "| {} | `{}` | `{}` | {} | {:.2}% | {:.4} | {:.4} | {} | {:.4} | {} | {} |",
            idx + 1,
            summary.strategy,
            summary.strategy_plugin,
            summary.total_trades,
            summary.win_rate * 100.0,
            summary.total_pnl,
            summary.total_stake,
            summary
                .turnover_yield
                .map(|v| format!("{:.2}%", v * 100.0))
                .unwrap_or_default(),
            summary.sharpe,
            summary.days_considered,
            summary
                .diagnostics
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    std::fs::write(path, lines.join("\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn summary_to_flat_row(summary: &BacktestSummary) -> Map<String, Value> {
    let mut row = Map::new();
    row.insert("backtest_id".into(), json!(summary.backtest_id));
    row.insert("strategy".into(), json!(summary.strategy));
    row.insert("strategy_plugin".into(), json!(summary.strategy_plugin));
    row.insert("city".into(), json!(summary.city));
    row.insert("date_from".into(), json!(summary.date_from));
    row.insert("date_to".into(), json!(summary.date_to));
    row.insert("total_trades".into(), json!(summary.total_trades));
    row.insert("wins".into(), json!(summary.wins));
    row.insert("win_rate".into(), json!(summary.win_rate));
    if summary.total_trades == 0 && summary.total_pnl == 0.0 {
        row.insert("total_pnl".into(), json!(0));
    } else {
        row.insert("total_pnl".into(), json!(summary.total_pnl));
    }
    row.insert("sharpe".into(), json!(summary.sharpe));
    row.insert("total_stake".into(), json!(summary.total_stake));
    row.insert("turnover_yield".into(), json!(summary.turnover_yield));
    row.insert(
        "pnl_per_calendar_day".into(),
        json!(summary.pnl_per_calendar_day),
    );
    row.insert(
        "pnl_per_active_day".into(),
        json!(summary.pnl_per_active_day),
    );
    row.insert("pnl_per_trade".into(), json!(summary.pnl_per_trade));
    row.insert("active_trade_days".into(), json!(summary.active_trade_days));
    row.insert(
        "return_on_daily_cap".into(),
        json!(summary.return_on_daily_cap),
    );
    row.insert("result_storage".into(), json!(summary.result_storage));
    row.insert("days_considered".into(), json!(summary.days_considered));
    row.insert(
        "days_with_forecast".into(),
        json!(summary.days_with_forecast),
    );
    row.insert(
        "days_with_price_history".into(),
        json!(summary.days_with_price_history),
    );
    row.insert(
        "days_with_minute_price_history".into(),
        json!(summary.days_with_minute_price_history),
    );
    row.insert(
        "days_with_snapshot_fills".into(),
        json!(summary.days_with_snapshot_fills),
    );
    row.insert(
        "days_missing_forecast".into(),
        json!(summary.days_missing_forecast),
    );
    row.insert(
        "days_missing_price_history".into(),
        json!(summary.days_missing_price_history),
    );
    row.insert("interval_minutes".into(), json!(summary.interval_minutes));
    row.insert(
        "interval_bars_considered".into(),
        json!(summary.interval_bars_considered),
    );
    row.insert(
        "intervals_with_trades".into(),
        json!(summary.intervals_with_trades),
    );
    row.insert("forecast_switches".into(), json!(summary.forecast_switches));
    for (key, value) in &summary.diagnostics {
        row.insert(format!("diagnostics.{key}"), json!(value));
    }
    if let Some(obj) = summary.params.as_object() {
        for (k, v) in obj {
            row.insert(format!("params.{k}"), v.clone());
        }
    }
    row
}

fn write_flat_rows(path: &Path, rows: &[Map<String, Value>], preferred: &[&str]) -> Result<()> {
    let mut keys = preferred.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut remaining = BTreeSet::<String>::new();
    for row in rows {
        for key in row.keys() {
            if !keys.contains(key) {
                remaining.insert(key.clone());
            }
        }
    }
    keys.extend(remaining);
    let mut writer = csv::Writer::from_path(path)?;
    writer.write_record(&keys)?;
    for row in rows {
        let record = keys
            .iter()
            .map(|key| value_for_csv(row.get(key)))
            .collect::<Vec<_>>();
        writer.write_record(record)?;
    }
    writer.flush()?;
    Ok(())
}

fn value_for_csv(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
    }
}

fn round4(v: f64) -> f64 {
    let rounded = (v * 10_000.0).round() / 10_000.0;
    if rounded == 0.0 { 0.0 } else { rounded }
}

fn round4_preserve_signed_zero(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn round6(v: f64) -> f64 {
    let rounded = (v * 1_000_000.0).round() / 1_000_000.0;
    if rounded == 0.0 { 0.0 } else { rounded }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_close_is_noon_utc() {
        assert_eq!(
            market_close_utc("2026-04-19").unwrap().to_rfc3339(),
            "2026-04-19T12:00:00+00:00"
        );
    }

    #[test]
    fn selects_forecast_by_priority_lead_distance_then_recency() {
        let evaluation_time = "2026-04-01T08:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let snapshots = vec![
            db::ForecastSnapshot {
                source: "open-meteo".into(),
                forecast_f: 70.0,
                gfs_f: None,
                ecmwf_f: Some(70.0),
                model_spread_c: None,
                forecast_lead_hours: Some(20.0),
                available_at: "2026-04-01T07:00:00Z".parse::<DateTime<Utc>>().unwrap(),
                priority: 1,
            },
            db::ForecastSnapshot {
                source: "gfs-global-24h".into(),
                forecast_f: 71.0,
                gfs_f: Some(71.0),
                ecmwf_f: None,
                model_spread_c: None,
                forecast_lead_hours: Some(27.0),
                available_at: "2026-04-01T06:00:00Z".parse::<DateTime<Utc>>().unwrap(),
                priority: 0,
            },
            db::ForecastSnapshot {
                source: "gfs-global-24h".into(),
                forecast_f: 72.0,
                gfs_f: Some(72.0),
                ecmwf_f: None,
                model_spread_c: None,
                forecast_lead_hours: Some(25.0),
                available_at: "2026-04-01T05:00:00Z".parse::<DateTime<Utc>>().unwrap(),
                priority: 0,
            },
        ];
        let selected = select_active_forecast(&snapshots, evaluation_time, 24.0).unwrap();
        assert_eq!(selected.forecast_f, 72.0);
    }
}
