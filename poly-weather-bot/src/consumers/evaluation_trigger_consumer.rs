use anyhow::Result;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use crate::core::bus::EventBus;
use crate::core::events::{EvaluationReason, EvaluationRequested, Event, EventSource};

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
                    Event::MetadataRefreshed(update) => {
                        self.publish(update.context.key.clone(), EvaluationReason::Startup);
                        self.publish(update.context.key, EvaluationReason::MarketMetadataChanged)
                    }
                    Event::ForecastUpdated(update) => {
                        self.publish(update.current.context_key, EvaluationReason::WeatherChanged)
                    }
                    Event::BestBidAskChanged(update) => self.publish(
                        update.context_key,
                        EvaluationReason::MarketPriceChanged {
                            token_id: update.token_id,
                        },
                    ),
                    Event::Stopping => break,
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

    fn publish(&self, context_key: String, reason: EvaluationReason) {
        self.bus.publish(
            EventSource::TriggerConsumer,
            Event::EvaluationRequested(EvaluationRequested {
                context_key,
                reason,
            }),
        );
    }
}
