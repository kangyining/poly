use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

use crate::core::bus::EventBus;
use crate::core::events::{Event, EventSource, StrategyEvent, SystemEvent};
use crate::core::state::SharedState;
use crate::services::StrategyService;

pub struct StrategyConsumer {
    bus: EventBus,
    state: SharedState,
    strategy_service: Arc<StrategyService>,
}

impl StrategyConsumer {
    pub fn new(bus: EventBus, state: SharedState, strategy_service: Arc<StrategyService>) -> Self {
        Self {
            bus,
            state,
            strategy_service,
        }
    }

    pub async fn consume(self) -> Result<()> {
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
                    warn!(dropped, "strategy consumer lagged behind the event stream");
                }
                Err(RecvError::Closed) => {
                    debug!("strategy consumer channel closed");
                    break;
                }
            }
        }

        Ok(())
    }
}
