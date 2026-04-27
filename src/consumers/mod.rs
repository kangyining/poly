pub mod evaluation_trigger_consumer;
pub mod execution_consumer;
pub mod state_consumer;
pub mod strategy_consumer;

use std::sync::Arc;

use tokio::task::JoinSet;

use crate::core::bus::EventBus;
use crate::core::state::SharedState;
use crate::services::{ExecutionService, StrategyService};

use self::evaluation_trigger_consumer::EvaluationTriggerConsumer;
use self::execution_consumer::ExecutionConsumer;
use self::state_consumer::StateConsumer;
use self::strategy_consumer::StrategyConsumer;

pub struct ConsumerServices {
    pub strategy: Arc<StrategyService>,
    pub execution: Arc<ExecutionService>,
}

pub fn spawn_all(
    tasks: &mut JoinSet<anyhow::Result<()>>,
    bus: &EventBus,
    state: &SharedState,
    services: ConsumerServices,
) {
    let bus_for_state = bus.clone();
    let state_for_state = state.clone();
    tasks.spawn(async move {
        StateConsumer::new(bus_for_state, state_for_state)
            .consume()
            .await
    });

    let bus_for_trigger = bus.clone();
    tasks.spawn(async move {
        EvaluationTriggerConsumer::new(bus_for_trigger)
            .consume()
            .await
    });

    let bus_for_strategy = bus.clone();
    let state_for_strategy = state.clone();
    let strategy_service = services.strategy.clone();
    tasks.spawn(async move {
        StrategyConsumer::new(bus_for_strategy, state_for_strategy, strategy_service)
            .consume()
            .await
    });

    let bus_for_execution = bus.clone();
    let state_for_execution = state.clone();
    let execution_service = services.execution.clone();
    tasks.spawn(async move {
        ExecutionConsumer::new(bus_for_execution, state_for_execution, execution_service)
            .consume()
            .await
    });
}
