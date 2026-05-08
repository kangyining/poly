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
use crate::services::{ExecutionService, PortfolioService, StrategyService};
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
        let contexts = self.config.contexts();
        let bus = EventBus::new(2048);
        let state = new_shared_state(contexts.len() * self.config.active_strategy_count());
        let weather_services = contexts
            .iter()
            .cloned()
            .map(|context| {
                Arc::new(WeatherService::new(
                    bus.clone(),
                    context,
                    self.config.forecast_refresh_secs,
                ))
            })
            .collect::<Vec<_>>();
        let market_services = contexts
            .iter()
            .cloned()
            .map(|context| {
                Arc::new(MarketService::new(
                    bus.clone(),
                    &self.config.gamma_url,
                    &self.config.clob_url,
                    context,
                ))
            })
            .collect::<Vec<_>>();

        let strategy_service = Arc::new(StrategyService::new(
            self.config.clone(),
            self.calibration,
            contexts.clone(),
            market_services.clone(),
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
                strategy: Arc::clone(&strategy_service),
                execution: Arc::clone(&execution_service),
                portfolio: Arc::clone(&portfolio_service),
            },
        );

        for market_service in &market_services {
            let initial_markets = market_service.publish_metadata().await?;
            let market_service = Arc::clone(market_service);
            tasks.spawn(async move { market_service.run_poller(initial_markets).await });
        }
        for weather_service in &weather_services {
            let weather_service = Arc::clone(weather_service);
            tasks.spawn(async move { weather_service.run_poller().await });
        }

        bus.publish(EventSource::App, Event::Started);
        info!(
            context_count = contexts.len(),
            "event-driven bot framework started"
        );

        if self.config.run_loop {
            tokio::signal::ctrl_c().await?;
            info!("received ctrl-c, stopping");
            bus.publish(EventSource::App, Event::Stopping);
        } else {
            loop {
                match control_rx.recv().await {
                    Ok(envelope) => match envelope.payload {
                        Event::OrderSubmitted(_) | Event::OrderSkipped(_) => break,
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
