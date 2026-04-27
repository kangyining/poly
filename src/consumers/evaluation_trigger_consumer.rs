use anyhow::Result;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use crate::core::bus::EventBus;
use crate::core::events::{
    EvaluationReason, EvaluationRequested, Event, EventSource, MarketEvent, StrategyEvent,
    SystemEvent, WeatherEvent,
};

pub struct EvaluationTriggerConsumer {
    bus: EventBus,
}

impl EvaluationTriggerConsumer {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }

    pub async fn consume(self) -> Result<()> {
        let mut rx = self.bus.subscribe();

        loop {
            match rx.recv().await {
                Ok(envelope) => match envelope.payload {
                    Event::System(SystemEvent::Started) => self.publish(EvaluationReason::Startup),
                    Event::Weather(WeatherEvent::ForecastUpdated(_)) => {
                        self.publish(EvaluationReason::WeatherChanged)
                    }
                    Event::Market(MarketEvent::MetadataRefreshed(_)) => {
                        self.publish(EvaluationReason::MarketMetadataChanged)
                    }
                    Event::Market(MarketEvent::BestBidAskChanged(update)) => {
                        self.publish(EvaluationReason::MarketPriceChanged {
                            token_id: update.token_id,
                        })
                    }
                    Event::System(SystemEvent::Stopping) => break,
                    _ => {}
                },
                Err(RecvError::Lagged(dropped)) => {
                    warn!(
                        dropped,
                        "evaluation trigger consumer lagged behind the event stream"
                    );
                }
                Err(RecvError::Closed) => {
                    debug!("evaluation trigger consumer channel closed");
                    break;
                }
            }
        }

        Ok(())
    }

    fn publish(&self, reason: EvaluationReason) {
        self.bus.publish(
            EventSource::TriggerConsumer,
            Event::Strategy(StrategyEvent::EvaluationRequested(EvaluationRequested {
                reason,
            })),
        );
    }
}
