use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

use crate::core::bus::EventBus;
use crate::core::events::{
    Event, EventSource, ExecutionPlanCreated, StrategyEvent, SystemEvent, TradingEvent,
};
use crate::core::state::SharedState;
use crate::services::ExecutionService;

pub struct ExecutionConsumer {
    bus: EventBus,
    state: SharedState,
    execution_service: Arc<ExecutionService>,
}

impl ExecutionConsumer {
    pub fn new(bus: EventBus, state: SharedState, execution_service: Arc<ExecutionService>) -> Self {
        Self {
            bus,
            state,
            execution_service,
        }
    }

    pub async fn consume(self) -> Result<()> {
        let mut rx = self.bus.subscribe();

        loop {
            match rx.recv().await {
                Ok(envelope) => match envelope.payload {
                    Event::Strategy(StrategyEvent::EvaluationCompleted(outcome)) => {
                        let new_token_ids = {
                            let state = self.state.read().await;
                            outcome
                                .plan
                                .trades
                                .iter()
                                .map(|trade| trade.no_token_id.clone())
                                .filter(|token_id| !state.execution.submitted_tokens.contains(token_id))
                                .collect::<Vec<_>>()
                        };

                        if new_token_ids.is_empty() {
                            self.bus.publish(
                                EventSource::ExecutionEngine,
                                Event::Trading(TradingEvent::OrderSkipped(crate::core::events::OrderSkipped {
                                    run_id: outcome.run_id.clone(),
                                    reason: "no new trades to submit".into(),
                                })),
                            );
                            continue;
                        }

                        self.bus.publish(
                            EventSource::ExecutionEngine,
                            Event::Trading(TradingEvent::ExecutionPlanCreated(
                                ExecutionPlanCreated {
                                    run_id: outcome.run_id.clone(),
                                    trade_count: outcome.plan.trades.len(),
                                    total_stake_usd: outcome.plan.total_stake_usd,
                                },
                            )),
                        );

                        info!(
                            run_id = %outcome.run_id,
                            trade_count = outcome.plan.trades.len(),
                            "submitting execution plan"
                        );
                        self.execution_service.execute(&outcome).await?;
                        self.bus.publish(
                            EventSource::ExecutionEngine,
                            Event::Trading(TradingEvent::OrderSubmitted(crate::core::events::OrderSubmitted {
                                run_id: outcome.run_id.clone(),
                                token_ids: new_token_ids,
                            })),
                        );
                    }
                    Event::System(SystemEvent::Stopping) => break,
                    _ => {}
                },
                Err(RecvError::Lagged(dropped)) => {
                    warn!(dropped, "execution consumer lagged behind the event stream");
                }
                Err(RecvError::Closed) => {
                    debug!("execution consumer channel closed");
                    break;
                }
            }
        }

        Ok(())
    }
}
