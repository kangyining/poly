# Polymarket Weather Bot Architecture

This codebase is an event-driven Rust bot for trading Polymarket weather range markets.
It watches two external domains:

- weather forecasts from the National Weather Service
- market metadata and prices from Polymarket

Those inputs are normalized into internal events, pushed through an in-process event bus, evaluated by a rule-based strategy engine, and converted into an execution plan that can be handed off to a Python execution bridge.

## High-Level Flow

```text
                +----------------------+
                |      App::run()      |
                | wires config/services|
                +----------+-----------+
                           |
                           v
                  +------------------+
                  |    EventBus      |
                  | tokio broadcast  |
                  +----+----+---+----+
                       |    |   | 
         publishes     |    |   |    consumes
                       |    |   |
     +-----------------+    |   +--------------------+
     |                      |                        |
     v                      v                        v
+-----------+        +-------------+          +---------------+
|WeatherSvc  |        | MarketSvc   |          |   Consumers   |
|REST poller |        | REST + WS   |          | state/trigger |
+-----+------+        +------+------+          | strategy/exec |
      |                      |                 +-------+-------+
      | ForecastUpdated      | MetadataRefreshed / BestBidAskChanged
      +----------+-----------+-------------------------+
                 |                                     |
                 v                                     v
          +----------------+                  +------------------+
          | Shared State   |                  | StrategyService  |
          | latest weather |                  | fetch markets +  |
          | market + exec  |                  | run rules        |
          +--------+-------+                  +--------+---------+
                   |                                   |
                   |                                   v
                   |                         EvaluationCompleted
                   |                                   |
                   |                                   v
                   |                         +------------------+
                   |                         | ExecutionConsumer|
                   |                         | dedupe + execute |
                   |                         +--------+---------+
                   |                                   |
                   |                                   v
                   |                         +------------------+
                   |                         | ExecutionService |
                   |                         | write JSON plan  |
                   |                         | call execute.py  |
                   |                         +------------------+
```

## Module Layout

```text
src/
  app.rs                Bootstraps the runtime and starts producers/consumers
  config.rs             CLI/env configuration
  core/
    bus.rs              In-process event bus
    events.rs           Event envelope and event types
    state.rs            Shared bot state snapshot
    logging.rs          Tracing setup
  weather/
    client.rs           NWS REST client and forecast models
    service.rs          Weather poller that publishes weather events
  market/
    client.rs           Polymarket REST clients and market parsing
    service.rs          Metadata fetch + websocket price stream publisher
  consumers/
    state_consumer.rs               Maintains shared state
    evaluation_trigger_consumer.rs  Converts source changes into strategy requests
    strategy_consumer.rs            Runs strategy evaluations
    execution_consumer.rs           Dedupes and hands plans to execution
  strategy/
    engine.rs           Rule pipeline executor
    rules.rs            Individual strategy rules
    calibration.rs      Forecast calibration lookup
    types.rs            Strategy inputs, traces, trade candidates
  services/
    event_service.rs    Target date and event slug helpers
    strategy_service.rs Orchestrates market fetch, calibration, and rule engine
    execution_service.rs Builds and executes combined execution plans
```

## Runtime Lifecycle

`App::run()` in [src/app.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/app.rs) is the entrypoint for the live system.

At startup it:

1. Expands configured cities and `days_in_future` into a set of `(city, target_date)` contexts.
2. Creates the `EventBus` and shared `BotState`.
3. Builds one `WeatherService` and one `MarketService` per context, plus shared strategy/execution services.
4. Spawns all consumers as long-running tasks.
5. Performs an initial market metadata fetch for every context and publishes it.
6. Starts the weather poller and market websocket poller for every context.
7. Publishes `SystemEvent::Started`.

In single-pass mode, the app exits after the first meaningful evaluation/execution cycle. In loop mode, it stays alive until `ctrl-c`, then publishes `SystemEvent::Stopping` so consumers can shut down cleanly.

## Event Bus Design

The bus lives in [src/core/bus.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/core/bus.rs). It is a thin wrapper around `tokio::sync::broadcast`.

Each published event is wrapped in an `EventEnvelope`:

- `id`: a monotonically increasing event id
- `at`: UTC timestamp
- `source`: where the event came from
- `payload`: the typed domain event

### Why broadcast?

Broadcast makes every consumer an independent subscriber. A single `ForecastUpdated` event can:

- update shared state
- trigger a strategy evaluation
- be logged or monitored by future consumers

without producers knowing who listens.

### Event categories

Event types are defined in [src/core/events.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/core/events.rs):

- `Market`
  - `MetadataRefreshed`
  - `BestBidAskChanged`
  - `ConnectionStatusChanged`
- `Weather`
  - `ForecastUpdated`
- `Strategy`
  - `EvaluationRequested`
  - `EvaluationCompleted`
- `Trading`
  - `ExecutionPlanCreated`
  - `OrderSubmitted`
  - `OrderSkipped`
- `System`
  - `Started`
  - `Stopping`
  - lag/health/error-style events

### Backpressure and lag

The bus has bounded capacity (`EventBus::new(2048)`). If a consumer falls behind, Tokio emits `RecvError::Lagged`. The state consumer reports this as a `SystemEvent::SourceLagged`; other consumers currently log the lag and continue.

## Publishers and Consumers

The architecture follows a clean split:

- publishers gather external data and publish domain events
- consumers react to events and advance the bot

### Publishers

#### Weather publisher

`WeatherService` in [src/weather/service.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/weather/service.rs):

- polls the forecast on an interval
- compares the newly fetched forecast with the previous one
- publishes `WeatherEvent::ForecastUpdated` only when the target-day high changes materially

Sources:

- `StaticForecastProvider` for manual overrides
- `NwsForecastProvider` for live data from `api.weather.gov`

#### Market publishers

`MarketService` in [src/market/service.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/market/service.rs) publishes two kinds of market information:

- metadata snapshots via `publish_metadata()`
- live quote changes via `run_poller()` and websocket subscriptions

It emits:

- `MarketEvent::MetadataRefreshed`
- `MarketEvent::BestBidAskChanged`
- `MarketEvent::ConnectionStatusChanged`

### Consumers

#### `StateConsumer`

[src/consumers/state_consumer.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/consumers/state_consumer.rs)

Responsibilities:

- maintains the in-memory `BotState`
- tracks lifecycle state such as `WarmingUp`, `Ready`, `Evaluating`, and `Executing`
- stores latest forecast, market metadata, best bids/asks, and submitted token ids

This consumer is the read model for the rest of the app.

#### `EvaluationTriggerConsumer`

[src/consumers/evaluation_trigger_consumer.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/consumers/evaluation_trigger_consumer.rs)

Responsibilities:

- listens for source changes
- converts them into `StrategyEvent::EvaluationRequested`

Triggers:

- app started
- weather updated
- market metadata refreshed
- market best bid/ask changed

This keeps producers simple. Weather and market services only publish facts; they do not decide when strategy should run.

#### `StrategyConsumer`

[src/consumers/strategy_consumer.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/consumers/strategy_consumer.rs)

Responsibilities:

- listens for `EvaluationRequested`
- reads the latest forecast from shared state
- skips evaluation until forecast data exists
- calls `StrategyService::evaluate(...)`
- publishes `EvaluationCompleted`

#### `ExecutionConsumer`

[src/consumers/execution_consumer.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/consumers/execution_consumer.rs)

Responsibilities:

- listens for `EvaluationCompleted`
- filters out trades whose `no_token_id` was already submitted
- publishes `ExecutionPlanCreated`
- calls `ExecutionService`
- publishes `OrderSubmitted` or `OrderSkipped`

This is the last event-driven step before interacting with the execution bridge.

## Shared State

Shared state lives in [src/core/state.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/core/state.rs) and is stored behind `Arc<RwLock<BotState>>`.

It holds:

- lifecycle state
- latest market metadata and best bid/ask maps
- latest weather forecast
- execution bookkeeping such as previously submitted token ids and the latest run id

This state is intentionally lightweight. It is not the system of record for trades; it is the in-memory snapshot that lets consumers coordinate.

## Rule and Strategy Execution

Strategy orchestration lives in [src/services/strategy_service.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/services/strategy_service.rs). The actual rule pipeline is in [src/strategy/engine.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/strategy/engine.rs) and [src/strategy/rules.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/strategy/rules.rs).

### Evaluation inputs

For each evaluation run, `StrategyService` builds a `StrategyInput` with:

- target forecast
- calibrated mean `mu_f = forecast_high_f + beta_f`
- calibrated sigma from `CalibrationTable`, clamped to `[0.8, 4.0]`
- EV threshold
- desired shares and stake sizing
- run metadata such as `run_id` and `event_slug`

It also re-fetches market data from Polymarket on each evaluation pass and excludes tokens already present in `open_positions.json`.

### Rule pipeline

The engine evaluates each market in order using a fixed rule chain:

1. `MarketTimingRule`
2. `TokenAvailabilityRule`
3. `BucketProbabilityRule`
4. `NoEdgeRule`

If any rule fails, the market is rejected immediately.

#### 1. `MarketTimingRule`

Allows trading only inside the 36-hour window before market close.

#### 2. `TokenAvailabilityRule`

Requires that the market has a `NO` token id that can actually be traded.

#### 3. `BucketProbabilityRule`

Computes the model probability that the forecasted high falls inside that market's temperature bucket using a Gaussian CDF.

This rule does not reject markets by itself; it records the probability into the decision trace for observability.

#### 4. `NoEdgeRule`

Computes fee-adjusted expected value for buying `NO`:

```text
EV_NO = (1 - P_model) * (1 - fee_rate * P_model) - effective_ask
```

A market qualifies only if `EV_NO >= min_ev_threshold`.

### Decision traces

For every evaluated market, the engine records:

- whether the market was accepted
- which rule passed or failed
- a human-readable reason string per rule

These traces are logged by `StrategyService` and are the main explainability layer for debugging why a bin qualified or was rejected.

### Candidate selection and sizing

Accepted markets become `TradeCandidate`s.

After the rule engine runs:

- candidates are summed by `stake_usd` per enabled strategy
- if a strategy exceeds its budget, candidates are trimmed in evaluation order until the cap is satisfied
- the combined execution plan is then trimmed again against the global `max_total_stake_usd` cap
- the remaining candidates are serialized into an `ExecutionPlan`

## Execution Flow

Execution has two layers:

### 1. Internal execution plan

[src/services/execution_service.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/services/execution_service.rs)

This converts the latest `TradeCandidate`s across all active city/date contexts into a JSON-serializable plan containing:

- run metadata
- dry-run flag
- total stake
- one planned trade per qualified market across the whole portfolio

Each planned trade includes `city_slug`, `target_date`, `event_slug`, the `NO` token id, ask price, shares, stake, modeled probability, EV, and fee rate.

`ExecutionService` writes one combined `execution_plan.json` and can then execute those trades directly.

## REST vs Websocket

This split is central to the architecture.

### Weather: REST only

Weather data comes from the National Weather Service via HTTP in [src/weather/client.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/weather/client.rs).

Why REST is used here:

- the bot only needs periodic forecast snapshots
- NWS exposes the needed forecast endpoints over HTTP
- forecast changes happen relatively slowly compared to market ticks

Flow:

1. call `/points/{lat},{lon}` to discover the hourly forecast URL
2. call the returned `forecastHourly` endpoint
3. compute the maximum temperature for the target date
4. publish `ForecastUpdated` if it changed

### Market: REST for discovery, websocket for live prices

Market data uses both protocols in [src/market/client.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/market/client.rs) and [src/market/service.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/market/service.rs).

REST is used for:

- fetching the event by slug from Gamma
- fetching each NO token order book from the CLOB API
- fetching fee rates from the CLOB API

Websocket is used for:

- subscribing to live market updates for all tracked NO token ids
- receiving best bid/ask or book updates in real time

Why the split exists:

- REST is the right tool for snapshot-style metadata and pre-trade enrichment
- websocket is the right tool for low-latency quote updates that should trigger re-evaluation quickly

In other words:

- use REST to learn what markets exist and what their current tradable economics look like
- use websocket to stay current on price movement after initialization

## Important Behavioral Notes

- `StrategyService` currently re-fetches market metadata and CLOB-derived pricing during each evaluation, even though the app also maintains websocket-driven market updates in shared state.
- websocket price updates trigger new evaluations, but the strategy engine evaluates against freshly fetched market snapshots rather than directly against the shared in-memory bid/ask maps.
- duplicate-order protection happens in two places:
  - open positions loaded from `open_positions.json` are filtered before evaluation
  - already submitted token ids in shared state are filtered before execution

This gives the bot a conservative bias against double-entering the same token.

## Typical End-to-End Event Sequence

```text
SystemEvent::Started
  -> EvaluationRequested(Startup)

MarketEvent::MetadataRefreshed
  -> StateConsumer stores markets
  -> EvaluationRequested(MarketMetadataChanged)

WeatherEvent::ForecastUpdated
  -> StateConsumer stores forecast
  -> EvaluationRequested(WeatherChanged)

StrategyEvent::EvaluationRequested
  -> StrategyConsumer calls StrategyService
  -> StrategyService fetches markets, runs rules, builds plan
  -> StrategyEvent::EvaluationCompleted

StrategyEvent::EvaluationCompleted
  -> ExecutionConsumer filters already-submitted tokens
  -> TradingEvent::ExecutionPlanCreated
  -> ExecutionService writes execution_plan.json
  -> TradingEvent::OrderSubmitted or TradingEvent::OrderSkipped
```

## Configuration Surfaces That Matter Architecturally

The most important config knobs in [src/config.rs](/Users/ryanqin/git/poly/poly-weather-bot/src/config.rs) are:

- `days_in_future` and optional `start_date`
- `[[cities]]` entries with `slug`, `station_code`, `lat`, `lon`
- per-city `forecast_high_f` to bypass live weather fetches
- `gamma_url`, `clob_url`
- `[[strategies]]` entries with `name`, `enabled`, `funding_pct`, `min_ev_threshold`, `desired_shares`, and `max_stake_usd`
- `max_total_stake_usd`
- `open_positions_path`, `execution_plan_dir`
- `dry_run`
- `run_loop`, `poll_interval_secs`

## Summary

This bot is best understood as four layers:

1. data ingestion from weather and market sources
2. internal event propagation over a broadcast bus
3. deterministic rule-based strategy evaluation
4. execution-plan generation plus Python handoff

That separation keeps external I/O, decision logic, and execution concerns decoupled, which makes the codebase easier to evolve and easier to debug when a trading decision looks surprising.
