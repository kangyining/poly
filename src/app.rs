use std::sync::Arc;

use anyhow::Result;
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::bus::EventBus;
use crate::calibration::CalibrationTable;
use crate::config::AppConfig;
use crate::events::{Event, EventSource, StrategyEvent, SystemEvent, TradingEvent};
use crate::listeners::evaluation_trigger_listener::EvaluationTriggerListener;
use crate::listeners::execution::ExecutionListener;
use crate::listeners::state::StateListener;
use crate::listeners::strategy_listener::StrategyListener;
use crate::polymarket::PolymarketClient;
use crate::services::{EventService, ExecutionService, StrategyService, WeatherService};
use crate::sources::market::MarketSource;
use crate::sources::weather::WeatherSource;
use crate::state::new_shared_state;

pub struct App {
    config: AppConfig,
    calibration: Option<CalibrationTable>,
}

impl App {
    pub fn new(config: AppConfig, calibration: Option<CalibrationTable>) -> Self {
        Self { config, calibration }
    }

    pub async fn run(self) -> Result<()> {
        let target_date = EventService::resolve_target_date(self.config.target_date);
        let event_slug = EventService::build_event_slug(&self.config.city, target_date);
        let bus = EventBus::new(2048);
        let state = new_shared_state();
        let weather_service = Arc::new(WeatherService::new(
            self.config.station_code.clone(),
            self.config.lat,
            self.config.lon,
            self.config.forecast_high_f,
        ));

        let market_client = PolymarketClient::new(&self.config.gamma_url, &self.config.clob_url);
        let market_source = MarketSource::new(
            bus.clone(),
            market_client.clone(),
            event_slug,
            target_date,
            self.config.desired_shares,
        );

        let strategy_service = Arc::new(StrategyService::new(
            self.config.clone(),
            self.calibration,
            market_client,
        ));
        let execution_service = Arc::new(ExecutionService::new(self.config.clone()));

        let weather_source = WeatherSource::new(
            bus.clone(),
            weather_service,
            target_date,
            self.config.poll_interval_secs,
        );

        let mut tasks = JoinSet::new();
        let mut control_rx = bus.subscribe();
        let bus_for_state = bus.clone();
        let state_for_state = state.clone();
        tasks.spawn(async move { StateListener::new(bus_for_state, state_for_state).run().await });
        let bus_for_eval_trigger = bus.clone();
        tasks.spawn(async move { EvaluationTriggerListener::new(bus_for_eval_trigger).run().await });
        let bus_for_strategy = bus.clone();
        let state_for_strategy = state.clone();
        let strategy_service = strategy_service.clone();
        tasks.spawn(async move {
            StrategyListener::new(bus_for_strategy, state_for_strategy, strategy_service)
                .run()
                .await
        });
        let bus_for_execution = bus.clone();
        let state_for_execution = state.clone();
        let execution_service = execution_service.clone();
        tasks.spawn(async move {
            ExecutionListener::new(bus_for_execution, state_for_execution, execution_service)
                .run()
                .await
        });
        let initial_markets = market_source.bootstrap_metadata().await?;
        let bus_for_weather = bus.clone();
        tasks.spawn(async move {
            let _ = bus_for_weather;
            weather_source.run().await
        });
        tasks.spawn(async move { market_source.run(initial_markets).await });

        bus.publish(EventSource::App, Event::System(SystemEvent::Started));
        info!("event-driven bot framework started");

        if self.config.run_loop {
            tokio::signal::ctrl_c().await?;
            info!("received ctrl-c, stopping");
            bus.publish(EventSource::App, Event::System(SystemEvent::Stopping));
        } else {
            loop {
                match control_rx.recv().await {
                    Ok(envelope) => match envelope.payload {
                        Event::Trading(TradingEvent::OrderSubmitted(_))
                        | Event::Trading(TradingEvent::OrderSkipped(_))
                        | Event::Strategy(StrategyEvent::EvaluationCompleted(_)) => break,
                        _ => {}
                    },
                    Err(error) => {
                        warn!(error = %error, "control subscriber stopped before first cycle completed");
                        break;
                    }
                }
            }
            bus.publish(EventSource::App, Event::System(SystemEvent::Stopping));
        }

        while let Some(task_result) = tasks.join_next().await {
            match task_result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => warn!(error = %error, "task exited with error"),
                Err(error) => warn!(error = %error, "task panicked"),
            }
        }

        Ok(())
    }
}
