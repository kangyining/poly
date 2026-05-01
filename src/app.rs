use std::sync::Arc;

use anyhow::Result;
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::consumers::{self, ConsumerServices};
use crate::core::bus::EventBus;
use crate::core::events::{Event, EventSource};
use crate::core::state::new_shared_state;
use crate::market::MarketService;
use crate::services::{EventService, ExecutionService, PortfolioService, StrategyService};
use crate::strategy::CalibrationTable;
use crate::weather::WeatherService;

pub struct App {
    config: AppConfig,
    calibration: Option<CalibrationTable>,
}

impl App {
    pub fn new(config: AppConfig, calibration: Option<CalibrationTable>) -> Self {
        Self {
            config,
            calibration,
        }
    }

    pub async fn run(self) -> Result<()> {
        let target_date = EventService::resolve_target_date(self.config.target_date);
        let event_slug = EventService::build_event_slug(&self.config.city, target_date);
        let bus = EventBus::new(2048);
        let state = new_shared_state();
        let weather_service = Arc::new(WeatherService::new(
            bus.clone(),
            self.config.station_code.clone(),
            self.config.lat,
            self.config.lon,
            self.config.forecast_high_f,
            target_date,
            self.config.forecast_refresh_secs,
        ));

        let market_service = Arc::new(MarketService::new(
            bus.clone(),
            &self.config.gamma_url,
            &self.config.clob_url,
            event_slug,
            target_date,
            self.config.desired_shares,
        ));

        let strategy_service = Arc::new(StrategyService::new(
            self.config.clone(),
            self.calibration,
            market_service.clone(),
        ));
        let execution_service = Arc::new(ExecutionService::new(self.config.clone()));
        let portfolio_service = Arc::new(PortfolioService::new(self.config.clone(), state.clone()));

        let mut tasks = JoinSet::new();
        let mut control_rx = bus.subscribe();
        consumers::spawn_all(
            &mut tasks,
            &bus,
            &state,
            ConsumerServices {
                strategy: strategy_service.clone(),
                execution: execution_service.clone(),
                portfolio: portfolio_service.clone(),
            },
        );
        let initial_markets = market_service.publish_metadata().await?;
        let weather_service = weather_service.clone();
        tasks.spawn(async move { weather_service.run_poller().await });
        let market_service = market_service.clone();
        tasks.spawn(async move { market_service.run_poller(initial_markets).await });

        bus.publish(EventSource::App, Event::Started);
        info!("event-driven bot framework started");

        if self.config.run_loop {
            tokio::signal::ctrl_c().await?;
            info!("received ctrl-c, stopping");
            bus.publish(EventSource::App, Event::Stopping);
        } else {
            loop {
                match control_rx.recv().await {
                    Ok(envelope) => match envelope.payload {
                        Event::OrderSubmitted(_)
                        | Event::OrderSkipped(_)
                        | Event::EvaluationCompleted(_) => break,
                        _ => {}
                    },
                    Err(error) => {
                        warn!(error = %error, "control subscriber stopped before first cycle completed");
                        break;
                    }
                }
            }
            bus.publish(EventSource::App, Event::Stopping);
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
