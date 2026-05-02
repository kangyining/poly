use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

use crate::core::bus::EventBus;
use crate::core::events::{Event, EventSource, ExecutionPlanCreated, OrderSkipped, OrderSubmitted};
use crate::core::state::SharedState;
use crate::services::{ExecutionPlan, ExecutionService};

pub struct ExecutionConsumer {
    bus: EventBus,
    state: SharedState,
    execution_service: Arc<ExecutionService>,
}

impl ExecutionConsumer {
    pub fn new(
        bus: EventBus,
        state: SharedState,
        execution_service: Arc<ExecutionService>,
    ) -> Self {
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
                    Event::EvaluationCompleted(_) => {
                        let (plan, new_token_ids, ready) = {
                            let state = self.state.read().await;
                            let ready = state.has_completed_initial_evaluations();
                            let outcomes = state
                                .execution
                                .latest_outcomes
                                .values()
                                .map(|outcome| outcome.as_ref())
                                .collect::<Vec<_>>();
                            let plan = ExecutionPlan::from_outcomes(
                                &outcomes,
                                self.execution_service.is_dry_run(),
                            );
                            let new_token_ids = plan
                                .trades
                                .iter()
                                .map(|trade| trade.no_token_id.clone())
                                .filter(|token_id| {
                                    !state.execution.submitted_tokens.contains(token_id)
                                })
                                .collect::<Vec<_>>();
                            (plan, new_token_ids, ready)
                        };

                        if !ready {
                            continue;
                        }

                        if new_token_ids.is_empty() {
                            self.bus.publish(
                                EventSource::ExecutionEngine,
                                Event::OrderSkipped(OrderSkipped {
                                    run_id: plan.run_id.clone(),
                                    reason: "no new trades to submit".into(),
                                }),
                            );
                            self.execution_service.write_plan(&plan)?;
                            continue;
                        }

                        self.bus.publish(
                            EventSource::ExecutionEngine,
                            Event::ExecutionPlanCreated(ExecutionPlanCreated {
                                run_id: plan.run_id.clone(),
                                trade_count: plan.trades.len(),
                                total_stake_usd: plan.total_stake_usd,
                            }),
                        );

                        info!(
                            run_id = %plan.run_id,
                            trade_count = plan.trades.len(),
                            "submitting combined execution plan"
                        );
                        let report = self.execution_service.execute(&plan).await?;
                        self.bus.publish(
                            EventSource::ExecutionEngine,
                            Event::PlanExecuted(Arc::new(report)),
                        );
                        self.bus.publish(
                            EventSource::ExecutionEngine,
                            Event::OrderSubmitted(OrderSubmitted {
                                run_id: plan.run_id.clone(),
                                token_ids: new_token_ids,
                            }),
                        );
                    }
                    Event::Stopping => break,
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
