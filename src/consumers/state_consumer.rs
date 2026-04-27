use anyhow::Result;
use chrono::Utc;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use crate::core::bus::EventBus;
use crate::core::events::{
    Event, EventSource, MarketEvent, StrategyEvent, SystemEvent, TradingEvent, WeatherEvent,
};
use crate::core::state::{LifecycleState, SharedState};

pub struct StateConsumer {
    bus: EventBus,
    state: SharedState,
}

impl StateConsumer {
    pub fn new(bus: EventBus, state: SharedState) -> Self {
        Self { bus, state }
    }

    pub async fn consume(self) -> Result<()> {
        let mut rx = self.bus.subscribe();

        loop {
            match rx.recv().await {
                Ok(envelope) => match envelope.payload {
                    Event::System(SystemEvent::Started) => {
                        let mut state = self.state.write().await;
                        state.lifecycle = LifecycleState::WarmingUp;
                    }
                    Event::Weather(WeatherEvent::ForecastUpdated(update)) => {
                        let mut state = self.state.write().await;
                        state.weather.forecast = Some(update.current);
                        state.weather.last_update_at = Some(Utc::now());
                        if state.is_ready() {
                            state.lifecycle = LifecycleState::Ready;
                        }
                    }
                    Event::Market(MarketEvent::MetadataRefreshed(update)) => {
                        let mut state = self.state.write().await;
                        state.market.event_slug = Some(update.event_slug);
                        state.market.target_date = Some(update.target_date);
                        state.market.markets = (*update.markets).clone();
                        state.market.last_metadata_at = Some(Utc::now());
                        if state.is_ready() {
                            state.lifecycle = LifecycleState::Ready;
                        }
                    }
                    Event::Market(MarketEvent::BestBidAskChanged(update)) => {
                        let mut state = self.state.write().await;
                        if let Some(best_bid) = update.best_bid {
                            state
                                .market
                                .best_bids
                                .insert(update.token_id.clone(), best_bid);
                        }
                        if let Some(best_ask) = update.best_ask {
                            state.market.best_asks.insert(update.token_id, best_ask);
                        }
                        state.market.last_price_event_at = Some(Utc::now());
                    }
                    Event::Strategy(StrategyEvent::EvaluationRequested(_)) => {
                        let mut state = self.state.write().await;
                        state.lifecycle = LifecycleState::Evaluating;
                    }
                    Event::Strategy(StrategyEvent::EvaluationCompleted(outcome)) => {
                        let mut state = self.state.write().await;
                        state.execution.last_run_id = Some(outcome.run_id.clone());
                        state.execution.last_evaluation_at = Some(Utc::now());
                        state.lifecycle = LifecycleState::Ready;
                    }
                    Event::Trading(TradingEvent::OrderSubmitted(update)) => {
                        let mut state = self.state.write().await;
                        for token_id in update.token_ids {
                            state.execution.submitted_tokens.insert(token_id);
                        }
                        state.execution.last_run_id = Some(update.run_id);
                        state.lifecycle = LifecycleState::Ready;
                    }
                    Event::Trading(TradingEvent::OrderSkipped(_)) => {
                        let mut state = self.state.write().await;
                        state.lifecycle = LifecycleState::Ready;
                    }
                    Event::Trading(TradingEvent::ExecutionPlanCreated(_)) => {
                        let mut state = self.state.write().await;
                        state.lifecycle = LifecycleState::Executing;
                    }
                    Event::System(SystemEvent::Stopping) => break,
                    _ => {}
                },
                Err(RecvError::Lagged(dropped)) => {
                    warn!(dropped, "state consumer lagged behind the event stream");
                    self.bus.publish(
                        EventSource::StateConsumer,
                        Event::System(SystemEvent::SourceLagged {
                            source: EventSource::StateConsumer,
                            dropped_messages: dropped,
                        }),
                    );
                }
                Err(RecvError::Closed) => {
                    debug!("state consumer channel closed");
                    break;
                }
            }
        }

        Ok(())
    }
}
