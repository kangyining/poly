use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use crate::core::bus::EventBus;
use crate::core::events::Event;
use crate::services::PortfolioService;

pub struct PortfolioConsumer {
    bus: EventBus,
    portfolio_service: Arc<PortfolioService>,
}

impl PortfolioConsumer {
    pub fn new(bus: EventBus, portfolio_service: Arc<PortfolioService>) -> Self {
        Self {
            bus,
            portfolio_service,
        }
    }

    pub async fn consume(self) -> Result<()> {
        let mut rx = self.bus.subscribe();

        loop {
            match rx.recv().await {
                Ok(envelope) => match envelope.payload {
                    Event::Started => {
                        self.portfolio_service.initialize().await?;
                    }
                    Event::PlanExecuted(report) => {
                        self.portfolio_service.record_execution(&report).await?;
                    }
                    Event::Stopping => break,
                    _ => {}
                },
                Err(RecvError::Lagged(dropped)) => {
                    warn!(dropped, "portfolio consumer lagged behind the event stream");
                }
                Err(RecvError::Closed) => {
                    debug!("portfolio consumer channel closed");
                    break;
                }
            }
        }

        Ok(())
    }
}
