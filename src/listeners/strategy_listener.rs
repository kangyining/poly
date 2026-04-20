use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

use crate::bus::EventBus;
use crate::events::{Event, EventSource, StrategyEvent, SystemEvent};
use crate::services::StrategyService;
use crate::state::SharedState;

pub struct StrategyListener {
    bus: EventBus,
    state: SharedState,
    strategy_service: Arc<StrategyService>,
}

impl StrategyListener {
    pub fn new(bus: EventBus, state: SharedState, strategy_service: Arc<StrategyService>) -> Self {
        Self {
            bus,
            state,
            strategy_service,
        }
    }

    pub async fn run(self) -> Result<()> {
        let mut rx = self.bus.subscribe();

        loop {
            match rx.recv().await {
                Ok(envelope) => match envelope.payload {
                    Event::Strategy(StrategyEvent::EvaluationRequested(request)) => {
                        let forecast = {
                            let state = self.state.read().await;
                            state.weather.forecast.clone()
                        };

                        let Some(forecast) = forecast else {
                            debug!(reason = ?request.reason, "skipping evaluation until forecast is available");
                            continue;
                        };

                        info!(reason = ?request.reason, "running strategy evaluation");
                        match self.strategy_service.evaluate(forecast).await {
                            Ok(outcome) => {
                                self.bus.publish(
                                    EventSource::StrategyEngine,
                                    Event::Strategy(StrategyEvent::EvaluationCompleted(Arc::new(
                                        outcome,
                                    ))),
                                );
                            }
                            Err(error) => {
                                warn!(error = %error, "strategy evaluation failed");
                            }
                        }
                    }
                    Event::System(SystemEvent::Stopping) => break,
                    _ => {}
                },
                Err(RecvError::Lagged(dropped)) => {
                    warn!(dropped, "strategy listener lagged behind the event stream");
                }
                Err(RecvError::Closed) => {
                    debug!("strategy listener channel closed");
                    break;
                }
            }
        }

        Ok(())
    }
}
