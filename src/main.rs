use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate, Utc};
use clap::Parser;
use serde::Deserialize;
use std::collections::HashSet;
use std::process::Command;
use tracing::{error, info, warn};

use polymarket_weather_bot::calibration::CalibrationTable;
use polymarket_weather_bot::config::{AppConfig, Cli};
use polymarket_weather_bot::execution::ExecutionPlan;
use polymarket_weather_bot::logging::init_logging;
use polymarket_weather_bot::polymarket::PolymarketClient;
use polymarket_weather_bot::strategy::{
    BucketProbabilityRule, MarketTimingRule, NoEdgeRule, StrategyEngine, StrategyInput,
    TokenAvailabilityRule,
};
use polymarket_weather_bot::weather::{
    market_close_utc, NwsForecastProvider, StaticForecastProvider,
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::try_from(cli)?;
    let _logging_guard = init_logging(&config.log_directory)?;

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

    let polymarket_client = PolymarketClient::new(&config.gamma_url, &config.clob_url);

    if config.run_loop {
        info!("starting continuous scheduler loop");
        loop {
            let target_date = resolve_target_date(config.target_date);
            let now = Utc::now();
            let close = market_close_utc(target_date);

            if now >= close {
                info!(target_date = %target_date, "market is closed, advancing to tomorrow");
                let next_open = market_close_utc(target_date + Duration::days(1)) - Duration::hours(36);
                let sleep_secs = (next_open - now).num_seconds().max(60) as u64;
                info!(sleep_secs, "sleeping until tomorrow's market opens");
                tokio::time::sleep(tokio::time::Duration::from_secs(sleep_secs)).await;
                continue;
            }

            match run_once(&config, &calib, &polymarket_client, target_date).await {
                Ok(plan) => {
                    info!(
                        trades = plan.trades.len(),
                        total_stake_usd = plan.total_stake_usd,
                        "run complete"
                    );
                    run_execute_bridge(&config);
                }
                Err(e) => {
                    error!(error = %e, "run failed, will retry next cycle");
                }
            }

            info!(secs = config.poll_interval_secs, "sleeping until next evaluation");
            tokio::time::sleep(tokio::time::Duration::from_secs(config.poll_interval_secs)).await;
        }
    } else {
        let target_date = resolve_target_date(config.target_date);
        let plan = run_once(&config, &calib, &polymarket_client, target_date).await?;
        info!(
            trades = plan.trades.len(),
            total_stake_usd = plan.total_stake_usd,
            dry_run = config.dry_run,
            "single run complete"
        );
        run_execute_bridge(&config);
    }

    Ok(())
}

async fn run_once(
    config: &AppConfig,
    calib: &Option<CalibrationTable>,
    client: &PolymarketClient,
    target_date: NaiveDate,
) -> Result<ExecutionPlan> {
    let run_id = format!("{}-{}", config.city, Utc::now().format("%Y%m%dT%H%M%SZ"));
    let event_slug = build_event_slug(&config.city, target_date);

    let forecast = if let Some(high_f) = config.forecast_high_f {
        info!(forecast_high_f = high_f, "using static forecast override");
        StaticForecastProvider::new(high_f).get_forecast(target_date, &config.station_code)?
    } else {
        info!(lat = config.lat, lon = config.lon, "fetching NWS live forecast");
        NwsForecastProvider::new(config.lat, config.lon)
            .get_forecast(target_date, &config.station_code)
            .await?
    };

    let (beta_f, sigma_f) = calib
        .as_ref()
        .map(|t| t.lookup(&config.station_code, target_date, forecast.lead_hours))
        .unwrap_or((0.0, 2.0));

    let mu_f = forecast.forecast_high_f + beta_f;
    let sigma_f = sigma_f.clamp(0.8, 4.0);

    info!(
        forecast_high_f = forecast.forecast_high_f,
        beta_f,
        mu_f,
        sigma_f,
        lead_hours = forecast.lead_hours,
        station = %config.station_code,
        "calibration applied"
    );

    let open_token_ids = load_open_positions(&config.open_positions_path);
    info!(open_positions = open_token_ids.len(), "open positions loaded");

    let markets = client
        .fetch_event_markets(&event_slug, target_date, config.desired_shares)
        .await?;

    let markets: Vec<_> = markets
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
        );
        plan.write_to_dir(&config.execution_plan_dir)?;
        return Ok(plan);
    }

    let strategy_input = StrategyInput {
        forecast: forecast.clone(),
        mu_f,
        sigma_f,
        min_ev_threshold: config.min_ev_threshold,
        desired_shares: config.desired_shares,
        order_size_usd: config.order_size_usd,
        run_id: run_id.clone(),
        event_slug: event_slug.clone(),
        evaluation_time_utc: None,
    };

    let engine = StrategyEngine::new(vec![
        Box::<MarketTimingRule>::default(),
        Box::<TokenAvailabilityRule>::default(),
        Box::<BucketProbabilityRule>::default(),
        Box::<NoEdgeRule>::default(),
    ]);

    let evaluation = engine.evaluate(&strategy_input, &markets);
    log_evaluation(&evaluation);

    let mut candidates = evaluation.candidates;
    let total_stake: f64 = candidates.iter().map(|c| c.stake_usd).sum();
    if total_stake > config.max_total_stake_usd {
        warn!(
            total_stake,
            max = config.max_total_stake_usd,
            candidate_count = candidates.len(),
            "total stake exceeds limit, trimming candidates"
        );
        let mut running = 0.0_f64;
        candidates.retain(|c| {
            if running + c.stake_usd <= config.max_total_stake_usd {
                running += c.stake_usd;
                true
            } else {
                false
            }
        });
        info!(retained = candidates.len(), "candidates after stake cap");
    }

    let plan = ExecutionPlan::from_candidates(
        &candidates,
        &run_id,
        &event_slug,
        &target_date.to_string(),
        config.dry_run,
    );
    plan.write_to_dir(&config.execution_plan_dir)?;

    Ok(plan)
}

fn run_execute_bridge(config: &AppConfig) {
    let script = &config.execute_script;
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
        .status();

    match status {
        Ok(s) if s.success() => info!("execute.py finished successfully"),
        Ok(s) => warn!(exit_code = ?s.code(), "execute.py exited with non-zero status"),
        Err(e) => warn!(error = %e, "failed to launch execute.py"),
    }
}

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

fn build_event_slug(city: &str, date: NaiveDate) -> String {
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

#[derive(Deserialize)]
struct OpenPosition {
    token_id: String,
    #[allow(dead_code)]
    status: String,
}

fn load_open_positions(path: &std::path::Path) -> HashSet<String> {
    if !path.exists() {
        return HashSet::new();
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
                HashSet::new()
            }
        },
        Err(e) => {
            warn!(error = %e, "failed to read open_positions.json, treating as empty");
            HashSet::new()
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
