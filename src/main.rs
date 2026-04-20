use anyhow::{ensure, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use clap::Parser;
use serde::Deserialize;
use std::process::Command;
use tracing::{error, info, warn};

use polymarket_weather_bot::calibration::CalibrationTable;
use polymarket_weather_bot::config::{AppConfig, Cli};
use polymarket_weather_bot::execution::ExecutionPlan;
use polymarket_weather_bot::logging::init_logging;
use polymarket_weather_bot::polymarket::PolymarketClient;
use polymarket_weather_bot::strategy::{
    BucketProbabilityRule, MarketTimingRule, NoEdgeRule, StrategyConfig, StrategyEngine,
    StrategyInput, StrategyRegistry, TokenAvailabilityRule,
};
use polymarket_weather_bot::weather::{
    market_close_utc, Forecast, NwsForecastProvider, StaticForecastProvider,
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::try_from(cli)?;
    let _logging_guard = init_logging(&config.log_directory)?;

    // Load calibration table once at startup
    let calib = if config.calibration_path.exists() {
        match CalibrationTable::load(&config.calibration_path) {
            Ok(t) => {
                info!(path = %config.calibration_path.display(), "calibration table loaded");
                Some(t)
            }
            Err(e) => {
                warn!(error = %e, "failed to load calibration, using defaults (beta=0, sigma=2)");
                None
            }
        }
    } else {
        warn!(
            path = %config.calibration_path.display(),
            "calibration.json not found, using defaults (beta=0, sigma=2)"
        );
        None
    };

    // Load strategy registry (optional — falls back to a single default strategy)
    let registry = if config.strategies_file.exists() {
        match StrategyRegistry::load(&config.strategies_file) {
            Ok(r) => {
                let active: Vec<_> = r.active().collect();
                info!(count = active.len(), path = %config.strategies_file.display(), "strategy registry loaded");
                r
            }
            Err(e) => {
                warn!(error = %e, "failed to load strategies.toml, using CLI params as default strategy");
                build_default_registry(&config)
            }
        }
    } else {
        info!(path = %config.strategies_file.display(), "strategies.toml not found, using CLI params as default strategy");
        build_default_registry(&config)
    };

    let polymarket_client = PolymarketClient::new(&config.gamma_url, &config.clob_url);

    if config.run_loop {
        // ── Continuous scheduler loop with split weather / market refresh ──
        info!(
            poll_interval_secs = config.poll_interval_secs,
            forecast_refresh_secs = config.forecast_refresh_secs,
            "starting continuous scheduler loop"
        );
        let mut cached_forecast: Option<CachedForecast> = None;
        loop {
            let target_date = resolve_target_date(config.target_date);
            let now = Utc::now();
            let close = market_close_utc(target_date);

            if now >= close {
                info!(
                    target_date = %target_date,
                    "market is closed, advancing to tomorrow"
                );
                // Sleep until next day's market opens (~24h before noon UTC)
                let next_open = market_close_utc(target_date + Duration::days(1))
                    - Duration::hours(36);
                let sleep_secs = (next_open - now).num_seconds().max(60) as u64;
                info!(sleep_secs, "sleeping until tomorrow's market opens");
                tokio::time::sleep(tokio::time::Duration::from_secs(sleep_secs)).await;
                continue;
            }

            let forecast =
                match get_forecast_for_cycle(&config, &mut cached_forecast, target_date).await {
                    Ok(forecast) => forecast,
                    Err(e) => {
                        error!(error = %e, "forecast unavailable, will retry next cycle");
                        info!(secs = config.poll_interval_secs, "sleeping until next evaluation");
                        tokio::time::sleep(tokio::time::Duration::from_secs(config.poll_interval_secs))
                            .await;
                        continue;
                    }
                };

            match run_once(
                &config,
                &calib,
                &polymarket_client,
                &registry,
                target_date,
                forecast,
            )
            .await
            {
                Ok(plan) => {
                    info!(
                        trades = plan.trades.len(),
                        total_stake_usd = plan.total_stake_usd,
                        "run complete"
                    );
                    if plan.trades.is_empty() {
                        info!("no trades generated; skipping execute bridge");
                    } else {
                        run_execute_bridge(&config);
                    }
                }
                Err(e) => {
                    error!(error = %e, "run failed, will retry next cycle");
                }
            }

            info!(secs = config.poll_interval_secs, "sleeping until next evaluation");
            tokio::time::sleep(tokio::time::Duration::from_secs(config.poll_interval_secs)).await;
        }
    } else {
        // ── Single pass ────────────────────────────────────────────────────
        let target_date = resolve_target_date(config.target_date);
        let forecast = fetch_forecast(&config, target_date).await?;
        let plan =
            run_once(&config, &calib, &polymarket_client, &registry, target_date, forecast).await?;
        info!(
            trades = plan.trades.len(),
            total_stake_usd = plan.total_stake_usd,
            dry_run = config.dry_run,
            "single run complete"
        );
        if plan.trades.is_empty() {
            info!("no trades generated; skipping execute bridge");
        } else {
            run_execute_bridge(&config);
        }
    }

    Ok(())
}

/// One full evaluation pass across all active strategies.
/// Markets are fetched once; each strategy filters and scores independently.
async fn run_once(
    config: &AppConfig,
    calib: &Option<CalibrationTable>,
    client: &PolymarketClient,
    registry: &StrategyRegistry,
    target_date: NaiveDate,
    forecast: Forecast,
) -> Result<ExecutionPlan> {
    let run_id = format!(
        "{}-{}",
        config.city,
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let event_slug = build_event_slug(&config.city, target_date);

    // ── Step 1: Forecast ───────────────────────────────────────────────────
    let mut forecast = forecast;
    ensure!(
        forecast.target_date == target_date,
        "forecast target date {} does not match evaluation target date {}",
        forecast.target_date,
        target_date
    );
    forecast.lead_hours = current_lead_hours(target_date);

    // ── Step 2: Calibration lookup ─────────────────────────────────────────
    let (beta_f, sigma_f) = calib
        .as_ref()
        .map(|t| t.lookup(&config.station_code, target_date, forecast.lead_hours))
        .unwrap_or((0.0, 2.0));

    let mu_f = forecast.forecast_high_f + beta_f;
    let sigma_f = sigma_f.max(0.8).min(4.0);

    info!(
        forecast_high_f = forecast.forecast_high_f,
        beta_f,
        mu_f,
        sigma_f,
        lead_hours = forecast.lead_hours,
        station = %config.station_code,
        "calibration applied"
    );

    // ── Step 3: Load open positions (skip already-held token IDs) ──────────
    let open_token_ids = load_open_positions(&config.open_positions_path);
    info!(open_positions = open_token_ids.len(), "open positions loaded");

    // ── Step 4: Fetch markets + CLOB books (shared across all strategies) ──
    let all_markets = client
        .fetch_event_markets(&event_slug, target_date, config.desired_shares)
        .await?;

    let available_markets: Vec<_> = all_markets
        .into_iter()
        .filter(|m| {
            if let Some(id) = &m.no_token_id {
                if open_token_ids.contains(id) {
                    info!(
                        market_id = %m.id,
                        no_token_id = %id,
                        "skipping: position already open"
                    );
                    return false;
                }
            }
            true
        })
        .collect();

    // ── Step 5: Risk check — max open positions ────────────────────────────
    if open_token_ids.len() >= config.max_open_positions {
        warn!(
            open = open_token_ids.len(),
            max = config.max_open_positions,
            "at max open positions, skipping new trades this run"
        );
        let plan = ExecutionPlan::from_candidates(
            &[],
            &run_id,
            &event_slug,
            &target_date.to_string(),
            config.dry_run,
            "default",
        );
        plan.write_to_dir(&config.execution_plan_dir)?;
        return Ok(plan);
    }

    // ── Step 6: Evaluate each active strategy ─────────────────────────────
    let engine = StrategyEngine::new(vec![
        Box::<MarketTimingRule>::default(),
        Box::<TokenAvailabilityRule>::default(),
        Box::<BucketProbabilityRule>::default(),
        Box::<NoEdgeRule>::default(),
    ]);

    let mut all_candidates = Vec::new();
    let mut total_run_stake = 0.0_f64;

    for strat in registry.active() {
        if total_run_stake >= config.max_total_stake_usd {
            warn!(strategy = %strat.name, "max total stake reached, skipping strategy");
            break;
        }

        let strategy_input = StrategyInput {
            forecast: forecast.clone(),
            mu_f,
            sigma_f,
            min_ev_threshold: strat.min_ev_threshold,
            desired_shares: strat.desired_shares,
            order_size_usd: config.order_size_usd * strat.funding_pct,
            run_id: run_id.clone(),
            event_slug: event_slug.clone(),
            evaluation_time_utc: None,
        };

        let evaluation = engine.evaluate(&strategy_input, &available_markets);
        log_evaluation(&evaluation);

        let mut candidates = evaluation.candidates;
        // Apply per-strategy stake cap
        let mut strategy_stake = 0.0_f64;
        candidates.retain(|c| {
            if strategy_stake + c.stake_usd <= strat.max_stake_usd
                && total_run_stake + strategy_stake + c.stake_usd <= config.max_total_stake_usd
            {
                strategy_stake += c.stake_usd;
                true
            } else {
                false
            }
        });

        info!(
            strategy = %strat.name,
            candidates = candidates.len(),
            strategy_stake,
            "strategy evaluation complete"
        );

        // Tag candidates with strategy name before collecting
        let mut tagged: Vec<_> = candidates
            .into_iter()
            .map(|c| (strat.name.clone(), c))
            .collect();
        total_run_stake += tagged.iter().map(|(_, c)| c.stake_usd).sum::<f64>();
        all_candidates.append(&mut tagged);
    }

    // ── Step 7: Build combined execution plan ─────────────────────────────
    // Flatten tagged candidates; strategy_name is embedded per-trade in the plan.
    // We build individual plans per strategy to keep the JSON clean, but merge them.
    use polymarket_weather_bot::execution::PlannedTrade;
    let trades: Vec<PlannedTrade> = all_candidates
        .iter()
        .map(|(strategy_name, c)| PlannedTrade {
            decision_id: c.decision_id.clone(),
            market_id: c.market_id.clone(),
            question: c.question.clone(),
            bin_label: c.range.label(),
            side: "NO",
            no_token_id: c.no_token_id.clone(),
            effective_ask: c.effective_ask,
            shares: c.shares,
            stake_usd: c.stake_usd,
            p_model: c.p_model,
            ev_no: c.ev_no,
            fee_rate: c.fee_rate,
            strategy_name: strategy_name.clone(),
        })
        .collect();

    let total_stake_usd = trades.iter().map(|t| t.stake_usd).sum();
    let plan = ExecutionPlan {
        run_id,
        event_slug,
        target_date: target_date.to_string(),
        total_stake_usd,
        dry_run: config.dry_run,
        trades,
    };
    plan.write_to_dir(&config.execution_plan_dir)?;

    Ok(plan)
}

#[derive(Clone)]
struct CachedForecast {
    forecast: Forecast,
    fetched_at: DateTime<Utc>,
}

async fn fetch_forecast(config: &AppConfig, target_date: NaiveDate) -> Result<Forecast> {
    if let Some(high_f) = config.forecast_high_f {
        info!(forecast_high_f = high_f, "using static forecast override");
        Ok(StaticForecastProvider::new(high_f).get_forecast(target_date, &config.station_code)?)
    } else {
        info!(lat = config.lat, lon = config.lon, "fetching NWS live forecast");
        NwsForecastProvider::new(config.lat, config.lon)
            .get_forecast(target_date, &config.station_code)
            .await
    }
}

async fn get_forecast_for_cycle(
    config: &AppConfig,
    cached_forecast: &mut Option<CachedForecast>,
    target_date: NaiveDate,
) -> Result<Forecast> {
    let now = Utc::now();
    let can_use_cached = cached_forecast
        .as_ref()
        .map(|cached| cached.forecast.target_date == target_date)
        .unwrap_or(false);
    let refresh_needed = match cached_forecast.as_ref() {
        Some(cached) if cached.forecast.target_date == target_date => {
            config.forecast_high_f.is_none()
                && (now - cached.fetched_at).num_seconds() >= config.forecast_refresh_secs as i64
        }
        _ => true,
    };

    if refresh_needed {
        match fetch_forecast(config, target_date).await {
            Ok(mut forecast) => {
                forecast.lead_hours = current_lead_hours(target_date);
                *cached_forecast = Some(CachedForecast {
                    forecast: forecast.clone(),
                    fetched_at: now,
                });
                info!(
                    target_date = %target_date,
                    forecast_high_f = forecast.forecast_high_f,
                    fetched_at = %now.to_rfc3339(),
                    "forecast cache refreshed"
                );
                return Ok(forecast);
            }
            Err(e) if can_use_cached => {
                let cached = cached_forecast
                    .as_ref()
                    .expect("cached forecast should exist when can_use_cached is true");
                let mut forecast = cached.forecast.clone();
                forecast.lead_hours = current_lead_hours(target_date);
                warn!(
                    error = %e,
                    target_date = %target_date,
                    cached_at = %cached.fetched_at.to_rfc3339(),
                    "forecast refresh failed; reusing cached forecast"
                );
                return Ok(forecast);
            }
            Err(e) => return Err(e),
        }
    }

    let cached = cached_forecast
        .as_ref()
        .expect("cached forecast should exist when refresh is not needed");
    let mut forecast = cached.forecast.clone();
    let age_secs = (now - cached.fetched_at).num_seconds().max(0);
    forecast.lead_hours = current_lead_hours(target_date);
    info!(
        target_date = %target_date,
        forecast_high_f = forecast.forecast_high_f,
        age_secs,
        "using cached forecast"
    );
    Ok(forecast)
}

fn build_default_registry(config: &AppConfig) -> StrategyRegistry {
    StrategyRegistry {
        strategies: vec![StrategyConfig {
            name: "default".to_string(),
            enabled: true,
            funding_pct: 1.0,
            min_ev_threshold: config.min_ev_threshold,
            desired_shares: config.desired_shares,
            max_stake_usd: config.max_total_stake_usd,
        }],
    }
}

// ── Execution bridge ──────────────────────────────────────────────────────────

/// Invoke execute.py synchronously so positions are written before the next loop
/// iteration. Skips silently if the script path is empty or does not exist.
fn run_execute_bridge(config: &polymarket_weather_bot::config::AppConfig) {
    let script = &config.execute_script;

    // Empty path = disabled
    if script.as_os_str().is_empty() {
        return;
    }
    if !script.exists() {
        warn!(path = %script.display(), "execute.py not found, skipping bridge");
        return;
    }

    let plan_path = config.execution_plan_dir.join("execution_plan.json");
    let positions_path = &config.open_positions_path;

    info!(script = %script.display(), "invoking execute.py");

    // Windows uses "python"; Linux/macOS use "python3"
    #[cfg(target_os = "windows")]
    let python_bin = "python";
    #[cfg(not(target_os = "windows"))]
    let python_bin = "python3";

    let status = Command::new(python_bin)
        .arg(script)
        .arg("--plan")
        .arg(&plan_path)
        .arg("--positions")
        .arg(positions_path)
        .arg("--log-dir")
        .arg(&config.log_directory)
        .status();

    match status {
        Ok(s) if s.success() => info!("execute.py finished successfully"),
        Ok(s) => warn!(exit_code = ?s.code(), "execute.py exited with non-zero status"),
        Err(e) => warn!(error = %e, "failed to launch execute.py — is Python on PATH?"),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns today if today's market is still open (before noon UTC), else tomorrow.
fn resolve_target_date(override_date: Option<NaiveDate>) -> NaiveDate {
    if let Some(d) = override_date {
        return d;
    }
    let now = Utc::now();
    let today = now.date_naive();
    if now < market_close_utc(today) {
        today
    } else {
        today + Duration::days(1)
    }
}

fn current_lead_hours(target_date: NaiveDate) -> f64 {
    ((market_close_utc(target_date) - Utc::now()).num_minutes() as f64 / 60.0).max(0.0)
}

/// Builds the Polymarket event slug, e.g. "highest-temperature-in-nyc-on-april-16-2026"
fn build_event_slug(city: &str, date: NaiveDate) -> String {
    let month = match date.month() {
        1  => "january",
        2  => "february",
        3  => "march",
        4  => "april",
        5  => "may",
        6  => "june",
        7  => "july",
        8  => "august",
        9  => "september",
        10 => "october",
        11 => "november",
        12 => "december",
        _  => "unknown",
    };
    format!(
        "highest-temperature-in-{}-on-{}-{}-{}",
        city,
        month,
        date.day(),
        date.year()
    )
}

/// Deserialize the open_positions.json exported by the Python bridge.
/// Returns a set of NO token IDs that already have open positions.
#[derive(Deserialize)]
struct OpenPosition {
    token_id: String,
    #[allow(dead_code)]
    status: String,
}

fn load_open_positions(path: &std::path::Path) -> std::collections::HashSet<String> {
    if !path.exists() {
        return std::collections::HashSet::new();
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Vec<OpenPosition>>(&text) {
            Ok(positions) => positions
                .into_iter()
                .filter(|p| p.status == "open")
                .map(|p| p.token_id)
                .collect(),
            Err(e) => {
                warn!(error = %e, "failed to parse open_positions.json, treating as empty");
                std::collections::HashSet::new()
            }
        },
        Err(e) => {
            warn!(error = %e, "failed to read open_positions.json, treating as empty");
            std::collections::HashSet::new()
        }
    }
}

fn log_evaluation(evaluation: &polymarket_weather_bot::strategy::StrategyEvaluation) {
    for market in &evaluation.evaluated_markets {
        for trace in &market.traces {
            info!(
                decision_id = %market.decision_id,
                market_id = %market.market_id,
                question = %market.question,
                accepted = market.accepted,
                rule = trace.rule_name,
                passed = trace.passed,
                reason = %trace.reason,
                "rule trace"
            );
        }
    }

    for candidate in &evaluation.candidates {
        info!(
            decision_id = %candidate.decision_id,
            market_id = %candidate.market_id,
            bin = %candidate.range.label(),
            p_model = candidate.p_model,
            ev_no = candidate.ev_no,
            effective_ask = candidate.effective_ask,
            shares = candidate.shares,
            stake_usd = candidate.stake_usd,
            "qualified trade candidate"
        );
    }
}
