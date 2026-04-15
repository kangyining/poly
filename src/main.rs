use anyhow::Result;
use clap::Parser;
use polymarket_weather_bot::config::{AppConfig, Cli};
use polymarket_weather_bot::execution::{
    DryRunExecutionClient, ExecutionClient, ExecutionPlan, PolymarketExecutionClient,
};
use polymarket_weather_bot::logging::init_logging;
use polymarket_weather_bot::polymarket::PolymarketClient;
use polymarket_weather_bot::strategy::{
    MarketTimingRule, NoPriceBandRule, StrategyEngine, StrategyEvaluation, StrategyInput,
    TemperatureDistanceRule, TokenAvailabilityRule,
};
use polymarket_weather_bot::weather::{ForecastProvider, StaticForecastProvider};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::try_from(cli)?;
    let _logging_guard = init_logging(&config.log_directory)?;

    let forecast_provider = StaticForecastProvider::new(config.forecast_high_f);
    let forecast = forecast_provider.get_forecast(config.target_date)?;

    let client = PolymarketClient::new(&config.gamma_url, &config.clob_url);
    let markets = client
        .fetch_event_markets(&config.event_slug, config.target_date)
        .await?;

    let strategy_input = StrategyInput {
        forecast,
        price_band: config.price_band,
        distance_threshold_f: config.distance_threshold_f,
        order_size_usd: config.order_size_usd,
        run_id: format!(
            "{}-{}",
            config.event_slug,
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        ),
        event_slug: config.event_slug.clone(),
        evaluation_time_utc: None,
    };

    let strategy_engine = StrategyEngine::new(vec![
        Box::<MarketTimingRule>::default(),
        Box::<TemperatureDistanceRule>::default(),
        Box::<NoPriceBandRule>::default(),
        Box::<TokenAvailabilityRule>::default(),
    ]);
    let evaluation = strategy_engine.evaluate(&strategy_input, &markets);
    let plan = ExecutionPlan::from_candidates(&evaluation.candidates);

    log_summary(&config, &strategy_input, &markets, &evaluation, &plan);

    if config.dry_run {
        DryRunExecutionClient.submit(&plan)?;
        info!("dry-run enabled; no orders submitted");
    } else {
        let live_client = PolymarketExecutionClient {
            api_key: None,
            api_secret: None,
            api_passphrase: None,
        };
        live_client.submit(&plan)?;
    }

    Ok(())
}

fn log_summary(
    config: &AppConfig,
    input: &StrategyInput,
    markets: &[polymarket_weather_bot::polymarket::WeatherMarket],
    evaluation: &StrategyEvaluation,
    plan: &ExecutionPlan,
) {
    info!(
        event_slug = %config.event_slug,
        forecast_target_date = %input.forecast.target_date,
        forecast_high_f = input.forecast.forecast_high_f,
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
