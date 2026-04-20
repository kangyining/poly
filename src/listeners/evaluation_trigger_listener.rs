use anyhow::Result;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use crate::bus::EventBus;
use crate::events::{
    EvaluationReason, EvaluationRequested, Event, EventSource, MarketEvent, StrategyEvent,
    SystemEvent, WeatherEvent,
};

pub struct EvaluationTriggerListener {
    bus: EventBus,
}

impl EvaluationTriggerListener {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }

    pub async fn run(self) -> Result<()> {
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
                    warn!(dropped, "evaluation trigger listener lagged behind the event stream");
                }
                Err(RecvError::Closed) => {
                    debug!("evaluation trigger listener channel closed");
                    break;
                }
            }
        }

        Ok(())
    }

    fn publish(&self, reason: EvaluationReason) {
        self.bus.publish(
            EventSource::TriggerListener,
            Event::Strategy(StrategyEvent::EvaluationRequested(EvaluationRequested {
                reason,
            })),
        );
    }
}
