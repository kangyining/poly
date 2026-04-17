use std::collections::HashSet;

use anyhow::Result;
use chrono::{Duration, Utc};
use tokio::time::{interval, MissedTickBehavior};
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::execution::{
    DryRunExecutionClient, ExecutionClient, ExecutionPlan, PolymarketExecutionClient,
};
use crate::market_data::PolymarketMarketDataClient;
use crate::polymarket::{PolymarketClient, WeatherMarket};
use crate::storage::JsonlStore;
use crate::strategy::{
    MarketTimingRule, NoPriceBandRule, StrategyEngine, StrategyEvaluation, StrategyInput,
    TemperatureDistanceRule, TokenAvailabilityRule,
};
use crate::weather::WeatherClient;

pub async fn run(config: &AppConfig, store: &JsonlStore) -> Result<()> {
    if config.monitor_mode {
        run_monitor_loop(config, store).await
    } else {
        let mut submitted_tokens = HashSet::new();
        run_cycle(config, store, &mut submitted_tokens).await
    }
}

async fn run_monitor_loop(
    config: &AppConfig,
    store: &JsonlStore,
) -> Result<()> {
    let mut ticker = interval(std::time::Duration::from_secs(config.poll_interval_seconds));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut submitted_tokens = HashSet::new();

    loop {
        ticker.tick().await;
        if let Err(error) = run_cycle(config, store, &mut submitted_tokens).await {
            warn!(error = ?error, "monitor cycle failed");
        }
    }
}

async fn run_cycle(
    config: &AppConfig,
    store: &JsonlStore,
    submitted_tokens: &mut HashSet<String>,
) -> Result<()> {
    let weather_client = WeatherClient::new();
    let forecast = weather_client.fetch_forecast(config.target_date).await?;
    let client = PolymarketClient::new();
    let mut markets = client
        .fetch_event_markets(&config.event_slug, config.target_date)
        .await?;
    overlay_websocket_prices(config, &mut markets).await?;

    let strategy_input = StrategyInput {
        forecast,
        price_band: config.price_band,
        distance_threshold_f: config.distance_threshold_f,
        order_size_usd: config.order_size_usd,
        run_id: format!(
            "{}-{}",
            config.event_slug,
            Utc::now().format("%Y%m%dT%H%M%SZ")
        ),
        event_slug: config.event_slug.clone(),
        evaluation_time_utc: Some(Utc::now()),
    };

    store.persist_run_started(&strategy_input, &strategy_input.forecast)?;

    let strategy_engine = StrategyEngine::new(vec![
        Box::<MarketTimingRule>::default(),
        Box::<TemperatureDistanceRule>::default(),
        Box::<NoPriceBandRule>::default(),
        Box::<TokenAvailabilityRule>::default(),
    ]);
    let evaluation = strategy_engine.evaluate(&strategy_input, &markets);
    store.persist_strategy_evaluation(&evaluation)?;

    let fresh_candidates = evaluation
        .candidates
        .iter()
        .filter(|candidate| !submitted_tokens.contains(candidate.no_token_id.as_deref().unwrap_or_default()))
        .cloned()
        .collect::<Vec<_>>();
    let plan = ExecutionPlan::from_candidates(&fresh_candidates);
    store.persist_execution_plan(&plan)?;

    log_summary(config, &strategy_input, &markets, &evaluation, &plan);

    if plan.trades.is_empty() {
        info!("no new trade intents for this cycle");
        return Ok(());
    }

    let report = if config.dry_run {
        DryRunExecutionClient.submit(&plan).await?
    } else {
        let live_client = PolymarketExecutionClient {
            private_key: config.polymarket_private_key.clone(),
        };
        live_client.submit(&plan).await?
    };

    for trade in &plan.trades {
        submitted_tokens.insert(trade.token_id.clone());
        store.persist_order_submission(
            &strategy_input.run_id,
            &trade.decision_id,
            &trade.token_id,
            &trade.market_id,
        )?;
    }
    store.persist_execution_report(&strategy_input.run_id, &report)?;

    Ok(())
}

async fn overlay_websocket_prices(
    config: &AppConfig,
    markets: &mut [WeatherMarket],
) -> Result<()> {
    let asset_ids: Vec<String> = markets
        .iter()
        .filter_map(|market| market.no_token_id.clone())
        .collect();

    let client = PolymarketMarketDataClient::new();
    let snapshot = client
        .sync_snapshot(
            &asset_ids,
            std::time::Duration::from_secs(config.orderbook_sync_seconds),
        )
        .await?;
    let updated = snapshot.update_market_prices(
        markets,
        Duration::seconds(config.max_book_staleness_seconds),
    );

    info!(
        updated_market_count = updated,
        asset_count = asset_ids.len(),
        "overlaid websocket NO prices onto event markets"
    );

    Ok(())
}

fn log_summary(
    config: &AppConfig,
    input: &StrategyInput,
    markets: &[WeatherMarket],
    evaluation: &StrategyEvaluation,
    plan: &ExecutionPlan,
) {
    info!(
        event_slug = %config.event_slug,
        forecast_target_date = %input.forecast.target_date,
        forecast_high_f = input.forecast.forecast_high_f,
        forecast_source = %input.forecast.source,
        market_count = markets.len(),
        price_min = config.price_band.min,
        price_max = config.price_band.max,
        distance_threshold_f = config.distance_threshold_f,
        mode = if config.dry_run { "dry-run" } else { "live" },
        "strategy inputs loaded"
    );

    if evaluation.candidates.is_empty() {
        warn!(
            event_slug = %config.event_slug,
            "no qualifying NO opportunities found"
        );
        return;
    }

    for candidate in &evaluation.candidates {
        info!(
            decision_id = %candidate.decision_id,
            market_id = %candidate.market_id,
            question = %candidate.question,
            range = %candidate.range.label(),
            distance_f = candidate.distance_f,
            no_price = candidate.no_price,
            stake_usd = candidate.stake_usd,
            "qualified trade candidate"
        );
    }

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
                "strategy rule trace"
            );
        }
    }

    info!(
        trade_count = plan.trades.len(),
        total_stake_usd = plan.total_stake_usd,
        mode = if config.dry_run { "dry-run" } else { "live" },
        "execution plan ready"
    );
}
