# Rust vs Python Cross-Comparison

This is a function-level comparison of implemented Rust surfaces in
`poly-weather-rust` against the current Python repo at `../poly-weather`.

Status legend:

- **Parity**: Intended to match Python behavior closely.
- **Near parity**: Same contract and intent, but golden artifact checks are still needed.
- **Partial**: Useful implementation exists, but Python does more.
- **Stub**: CLI/function surface exists but behavior is intentionally not ported.
- **Rust-only**: Support type/helper with no direct Python function.
- **Python-only gap**: Python behavior has no Rust implementation yet.

## Top-Level CLI

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `main.rs::Cli` | separate Python scripts | Rust-only | One subcommand CLI replaces multiple Python entry scripts. |
| `main.rs::Command` | separate Python scripts | Rust-only | Enumerates all migrated command surfaces. |
| `main.rs::main` | script `main()` functions | Partial | Dispatches implemented Rust commands. Python still owns recorder/backfill breadth. |

## Command Modules

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `commands::backtest::BacktestArgs` | `analysis/phase1_backtest.py::main` parser | Near parity | Preserves strategy/date/city/db/artifact/list flags. |
| `commands::backtest::run` | `backtest.py`, `analysis/phase1_backtest.py::main` | Near parity | Runs backtests and writes summary/trade/report artifacts. Exact artifact byte parity pending. |
| `commands::backtest::parse_date_or` | inline date parsing | Parity | Same default-yesterday/date override intent. |
| `commands::bankroll::BankrollArgs` | `analysis/bankroll_backtest.py::main` parser | Near parity | Includes bankroll, scale-down, and artifact flags. |
| `commands::bankroll::run` | `run_bankroll_backtest`, `main` | Near parity | Runs Rust backtest candidates through bankroll ledger and writes artifacts. |
| `commands::bankroll::auto_paths` | `_auto_bankroll_paths` | Near parity | Same output set; filename stem may differ. |
| `commands::bankroll::write_summary_json` | JSON write block in `main` | Near parity | Writes combined summary JSON. |
| `commands::bankroll::write_summary_csv` | `_write_csv(...summary...)` | Near parity | Preferred columns match the Python summary intent; exact ordering needs golden checks. |
| `commands::bankroll::write_report` | `_write_report` | Near parity | Same report sections/metrics, not byte-identical. |
| `commands::bankroll::combined_summary` | `run_bankroll_backtest` summary merge | Near parity | Merges backtest scope and ledger metrics. |
| `commands::bankroll::write_rows` | `_write_csv` | Near parity | Generic CSV serialization. |
| `commands::bankroll::write_maps` | `_write_csv` with flattened dicts | Near parity | Generic map writer. |
| `commands::sweep::RankBy` | `--rank-by` choices | Parity | ROI/PnL/Sharpe/win-rate rank options. |
| `commands::sweep::Preset` | `PRESETS` names | Near parity | Rust has quick/full/exhaustive presets. Grid values still need golden comparison. |
| `commands::sweep::SweepArgs` | `param_sweep.py` parser | Near parity | Accepts Python-compatible sweep flags. |
| `commands::sweep::run` | `param_sweep.py::main` | Near parity | Runs sweep tasks with threaded `--jobs` workers. |
| `commands::sweep::iter_param_grid` | `iter_param_grid` | Near parity | Expands common, preset, global, and strategy-specific grids. |
| `commands::sweep::selected_strategies` | `strategy_names` | Near parity | Supports all/group/backtest-ready/default selection. |
| `commands::sweep::preset_values` | `apply_preset` / `PRESETS` | Near parity | Quick/full/exhaustive values match intent; exact Python grid parity needs golden checks. |
| `commands::sweep::parse_float_list` | `parse_float_list` | Near parity | Supports comma lists and inclusive `start:stop:step` ranges. |
| `commands::sweep::parse_param_grid` | `parse_key_float_list` | Parity | Parses `KEY=v1,v2`. |
| `commands::sweep::parse_strategy_param_grid` | `parse_strategy_key_float_list` | Parity | Parses `STRATEGY:KEY=v1,v2`. |
| `commands::sweep::cartesian` | `itertools.product` | Parity | Builds grid combinations. |
| `commands::sweep::preset_extra_grid` | `STRATEGY_EXTRA_GRIDS` | Near parity | Ports quick/full/exhaustive strategy extras. |
| `commands::sweep::metric_from_values` | `metric_value` | Near parity | Applies min-trade filter and selected metric. |
| `commands::sweep::write_sweep_csv` | `write_csv` | Near parity | Writes row CSV; field order differs from Python. |
| `commands::sweep::write_sweep_report` | `write_report` | Near parity | Same top-results purpose, simplified text. |
| `commands::sweep::write_strategy_artifacts` | `write_strategy_artifacts` | Near parity | Per-strategy CSV/Markdown artifacts. |
| `commands::sweep::write_best_by_strategy_report` | `write_final_strategy_report` | Near parity | Best-row comparison report. |
| `commands::sweep::available_date_range` | `available_dates` | Partial | Uses min/max settlement range, not Python's per-date forecast/price validation list. |
| `commands::calibrate::CalibrateArgs` | `calibrate.py` parser | Near parity | Supports DB and output path. |
| `commands::calibrate::run` | `analysis/calibration.py::main` | Near parity | Builds and writes calibration JSON. |
| `commands::execute::ExecuteArgs` | `execute.py` parser | Partial | Supports plan/positions/db/dry-run. Live client flags are intentionally absent. |
| `commands::execute::ExecutionPlan` | `load_plan` result dict | Near parity | Typed plan parser for current execution plan shape. |
| `commands::execute::PlannedTrade` | per-trade plan dict | Near parity | Typed dry-run/paper trade input. |
| `commands::execute::default_true` | Python default dry-run behavior | Parity | Plan dry-run defaults true. |
| `commands::execute::run` | `execute.py::main` | Partial | Parses plan, rejects live, inserts paper positions, exports open positions. No live submission. |
| `commands::settle::SettleArgs` | `settle.py` / `settlement_tracker.py` parser | Partial | Date/backfill/db/positions support. |
| `commands::settle::run` | `settlement_tracker.daily_check/backfill` | Partial | Fetches Gamma outcomes, inserts settlements, closes paper positions. |
| `commands::settle::fetch_event_by_slug` | `gamma_client.fetch_event_by_slug` | Near parity | Uses Gamma events endpoint by slug. |
| `commands::settle::winning_bin` | `gamma_client.winning_bin` | Near parity | Detects settled winning market from outcome prices. |
| `commands::settle::outcome_snapshot` | settlement raw outcome snapshot | Near parity | Captures label to prices JSON. |
| `commands::settle::parse_bin_midpoint` | `settlement_tracker._parse_bin_midpoint` | Near parity | Estimates midpoint from label; edge labels are approximated. |
| `commands::settle::settle_positions` | `settlement_tracker._settle_positions` | Near parity | Paper NO PnL closeout. Live/order reconciliation is out of scope. |
| `commands::status::StatusArgs` | `status.py` parser | Near parity | Open/closed/date/db flags. |
| `commands::status::run` | `status.py::main` | Near parity | Reads positions and prints totals. Formatting not byte-identical. |
| `commands::recorder::RecorderArgs` | `run_recorder.py` and data scripts parsers | Partial | Flags are present, plus Rust `--db`. |
| `commands::recorder::run` | `run_recorder.py::main` | Near parity | `--once`, continuous scheduling, TAF/NWS, Open-Meteo, market snapshots, and backfill dispatch are ported. |

## Config Module

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `WeatherContractConfig` | config dict rows in `weather_contracts.py` | Rust-only | Typed static config row. |
| `enabled_contracts` | `enabled_daily_high_contracts` | Parity | Filters enabled contracts. |
| `normalize_key` | `_normalize_key` | Near parity | Same lowercase slug normalization intent. |
| `contract_for_city` | `contract_for_city` | Parity | Matches slug/display aliases. |
| `contract_for_station` | `contract_for_station` | Parity | Matches enabled config by settlement station ID. |
| `normalize_city` | `normalize_contract_city`, `_normalize_city_filter` | Near parity | Returns configured display city. Unknown fallback differs slightly by caller. |
| `station_for_city` | `_station_for_city` | Parity | Configured station lookup. |
| `known_city_names` | `_known_city_names` | Parity | Enabled display names. |
| `enabled_station_ids` | `enabled_station_ids` | Parity | Sorted enabled station list. |
| `build_daily_high_slug` | `data/weather_contracts.py::build_daily_high_slug` | Parity | Same `highest-temperature-in-{city}-on-{month}-{day}-{year}` shape. |
| `default_event_slug` | `_default_event_slug` | Parity | Builds default slug from city/date. |
| `month_name` | Python `strftime("%B").lower()` | Parity | Explicit month map. |

Python config gaps not yet in Rust:

- `contract_for_display_city`

## DB Module

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `Database` | `data/db.py::Database` | Partial | Opens read-only/mutable and applies schema. Python has more migration helpers. |
| `Database::open` | `Database.__init__` | Partial | Read-only or mutable open. Mutable open applies copied schema; Python also applies migrations. |
| `Database::conn` | `Database.conn` attribute | Rust-only | Exposes the underlying connection for typed query helpers. |
| `SettlementRow` | settlement row dict/model | Rust-only | Typed query row. |
| `ForecastSnapshot` | forecast snapshot dict | Rust-only | Typed query row. |
| `PriceBar` | interval price row dict | Rust-only | Typed query row. |
| `SnapshotFill` | snapshot fill dict | Rust-only | Typed query row. |
| `PositionRow` | position row dict | Rust-only | Typed query row. |
| `NewPosition` | position insert dict | Rust-only | Typed insert payload. |
| `NewSettlement` | `SettlementRecord`/insert dict | Rust-only | Typed insert payload. |
| `parse_ts` | `_parse_iso_timestamp` | Near parity | RFC3339 and simple naive timestamps. Python accepts more loose ISO variants. |
| `parse_gfs_model_run` | `_parse_gfs_model_run` | Parity | Reads `past_model_run` from raw JSON and normalizes UTC. |
| `settlement_from_row` | inline row conversion | Rust-only | Internal mapper. |
| `load_settlements` | `_run_interval_backtest` settlement SQL | Parity | Date/city filtering. |
| `load_forecast_snapshots` | `_load_forecast_snapshots` | Near parity | Sources, model-run timestamp, and priority are ported. |
| `OrderLevel` | order level dict | Rust-only | Internal deserialization helper. |
| `value_as_f64` | inline float/string parsing | Rust-only | Internal helper. |
| `effective_ask_from_levels` | `_effective_ask_from_levels` | Parity | VWAP fill across ask ladder. |
| `load_snapshot_fill_bars` | `_load_snapshot_fill_bars` | Near parity | Snapshot VWAP/no_vwap/best-ask fallback ported. Needs fixture parity. |
| `floor_timestamp` | `_floor_timestamp` | Parity | Floors to interval bucket. |
| `load_price_bars` | `_load_interval_price_bars` | Parity | Reads `price_history_1m` in 36h window. |
| `load_metar_records` | `_load_metar_records` | Near parity | Same rows, direct scan instead of prefix optimization. |
| `load_climatology_temps` | `_climatology_temps` | Parity | Calendar-neighbor settlement highs. |
| `load_positions` | `status.py` SQL | Near parity | Status/date filters and row mapping. |
| `upsert_weather_contract` | `Database.upsert_weather_contract` | Near parity | Inserts/updates normalized contract metadata. |
| `insert_market_snapshot` | `Database.insert_snapshot` | Near parity | Inserts market snapshot rows with compatible columns. |
| `insert_metar` | `Database.insert_metar` | Near parity | Inserts METAR observations. |
| `insert_forecast` | `Database.insert_forecast` | Near parity | Inserts Open-Meteo forecast records. |
| `insert_forecast_model_run` | `Database.insert_forecast_model_run` | Near parity | Inserts per-model forecast rows. |
| `insert_position` | `execute.py`/DB insert | Near parity | Paper insert path. Duplicate behavior depends on schema. |
| `get_open_positions` | open-position query | Near parity | Convenience wrapper. |
| `close_position` | `settlement_tracker._settle_positions` update | Near parity | Updates status/PnL/settled time. |
| `export_open_positions_json` | `open_positions.json` export | Near parity | Preserves shape for open paper positions. |
| `settlement_exists` | `settlement_tracker.backfill` skip check | Parity | City/date existence check. |
| `insert_settlement` | settlement insert/replace | Near parity | Uses compatible schema columns. |
| `scalar_count` | ad hoc count queries | Rust-only | Utility helper. |

Python DB/data gaps still in Rust:

- Broad auxiliary migrations still rely mostly on the copied schema.
- Some Python retry/rate-limit policies around TAF/NWS/ASOS/price-history jobs
  are simplified.

## Strategy Core

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `ForecastState` | `ForecastState` dataclass | Parity | Field-compatible for implemented strategies. |
| `MarketBin` | `MarketBin` dataclass | Parity | Field-compatible for implemented strategies. |
| `StrategyContext` | `StrategyContext` dataclass | Parity | Field-compatible for implemented strategies. |
| `StrategyCandidate` | `StrategyCandidate` dataclass | Near parity | Same semantic fields. Python has `to_trade`; Rust converts in backtest. |
| `StrategyDiagnostic` | `StrategyDiagnostic` dataclass | Parity | Same diagnostic contract. |
| `StrategyOutput` | `StrategyOutput` dataclass | Parity | Candidate/diagnostic container. |
| `Strategy` trait | `Strategy` protocol | Near parity | Rust trait replaces Python protocol/base class. |
| `Strategy::name` | `BaseStrategy.name` / class name usage | Near parity | Implemented by every Rust strategy plugin. |
| `Strategy::required_data` | `BaseStrategy.required_data` | Near parity | Implemented or defaulted by Rust plugins. |
| `Strategy::generate_candidates` | `generate_candidates` methods | Near parity | Implemented by every executable/placeholder plugin. |
| `missing` | `BaseStrategy.missing` | Parity | Explicit missing-data diagnostic output. |

## Strategy Utilities

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `TemperatureRange` | `TemperatureRange` dataclass | Parity | Same below/above/between categories. |
| `parse_temperature_range` | `parse_temperature_range` | Near parity | Supports common labels; Python regex is somewhat more permissive. |
| `norm_cdf` | `norm_cdf` | Parity | Uses `libm::erf`, matching Python `math.erf` numerically within normal tolerance. |
| `bucket_probability` | `bucket_probability` | Near parity | Same boundary math; depends on label parser parity. |
| `entry_cost` | `entry_cost` | Parity | Same fee-adjusted cost formula. |
| `settle_single` | `settle_single` | Parity | Same YES/NO PnL. |
| `settle_union` | `settle_union` | Parity | Same union PnL. |
| `param_f64` | inline dict access | Rust-only | Typed helper. |
| `param_opt_f64` | inline dict access | Rust-only | Typed helper. |
| `param_bool` | inline dict access | Rust-only | Typed helper. |
| `min_edge` | `min_edge` | Parity | Same `min_ev_threshold` then `min_edge` fallback. |
| `candidate_size` | `candidate_size` | Near parity | Supports fixed stake and desired-shares sizing. Python also has separate `candidate_shares`. |
| `range_contains` | inline range checks | Rust-only | Shared helper. |

## Strategy Registry

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `StrategyConfigFile` | strategy config dict | Rust-only | Typed JSON wrapper. |
| `load_config` | `_load_config` | Parity | Reads `strategies.json`. |
| `list_strategy_configs` | `list_strategy_configs` | Near parity | Metadata shape/order may differ slightly. |
| `group_members` | `group_members` | Parity | Reads configured group. |
| `all_members` | `group_members("all")` | Parity | Convenience wrapper. |
| `resolve_strategy_name` | `resolve_strategy_name` | Parity | Supports `strategy_id` and `base_ev -> base_no_ev`. |
| `load_strategy_params` | `load_strategy_params` | Near parity | Merges defaults and per-strategy overrides. No legacy TOML fallback. |

Python registry gap:

- `get_strategy` exists in Python; Rust equivalent is `plugins::strategy_by_name`.
- `list_strategy_names` is not separately implemented in Rust.

## Strategy Plugins

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `event_slug` | `_event_slug` | Near parity | Uses context/default slug. |
| `market_id` | `_market_id` | Parity | Same synthetic ID intent. |
| `token` | `_token` | Parity | Same synthetic token intent. |
| `single_candidate` | `_single_candidate` | Near parity | Same candidate construction. Metadata ordering differs by JSON map behavior. |
| `metadata` | inline dict construction | Rust-only | Internal helper. |
| `BaseNoEvStrategy` | `BaseNoEvStrategy` | Near parity | Same calibrated NO EV strategy. |
| `BaseYesNoEvStrategy` | `BaseYesNoEvStrategy` | Near parity | Same YES/NO EV scan. |
| `generate_yes_no` | shared `BaseYesNoEvStrategy` logic | Rust-only | Internal shared implementation for YES/NO-capable plugins. |
| `EnsembleBucketEvStrategy` | `EnsembleBucketEvStrategy` | Near parity | Same ensemble averaging intent. |
| `DynamicSigmaEvStrategy` | `DynamicSigmaEvStrategy` | Near parity | Same sigma adjustment intent. |
| `BayesianClimatologyEvStrategy` | `BayesianClimatologyEvStrategy` | Near parity | Same model/climatology blending intent. |
| `CorridorLadderStrategy` | `CorridorLadderStrategy` | Near parity | Same corridor candidate generation. |
| `ThresholdUnionStrategy` | `ThresholdUnionStrategy` | Near parity | Same tail union strategy. |
| `BasketArbStrategy` | `BasketArbStrategy` | Near parity | Same basket-cost/edge scan. |
| `MakerQuoteSimStrategy` | `MakerQuoteSimStrategy` | Near parity | Same quote simulation/proxy intent. Snapshot bid parity still needs fixtures. |
| `NowcastMetarStrategy` | `NowcastMetarStrategy` | Near parity | Same METAR-adjusted logic. |
| `UnsupportedResearchStrategy` | `UnsupportedResearchStrategy` | Parity | Explicit missing-data diagnostics for research placeholders. |
| `strategy_by_name` | `all_strategy_instances` / `get_strategy` | Near parity | Resolves implemented and placeholder strategies. Unknown defaults to base strategy instead of raising. |

## Backtest Analysis

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `BacktestRequest` | `run_backtest` args | Rust-only | Typed request. |
| `BacktestTrade` | trade dict | Rust-only | Internal summary trade. |
| `BacktestSummary` | summary dict | Rust-only | Typed summary plus optional trade rows. |
| `BacktestTradeRow` | backtest trade row dict | Rust-only | Artifact-compatible trade row. |
| `run_backtest` | `run_backtest`, `_run_interval_backtest` | Near parity | Core interval replay, fills, strategies, diagnostics, artifacts. Golden parity pending. |
| `group_price_bars` | interval grouping in `_load_interval_price_bars` | Parity | Groups bars by floored interval timestamp. |
| `floor_timestamp` | `_floor_timestamp` | Parity | Same bucket flooring. |
| `select_active_forecast` | `_select_active_forecast` | Near parity | Priority, exact lead-distance, recency sorting. Needs fixture checks. |
| `build_market_bin` | `_build_market_bin` | Near parity | Snapshot fill path and price-history proxy path ported. |
| `market_close_utc` | `_market_close_utc` | Parity | Noon UTC close. |
| `market_open_utc` | `_market_open_utc` | Parity | 36h before close. |
| `metar_max_before` | `_metar_max_before` / `_metar_max_from_prefix` | Near parity | Direct scan instead of prefix arrays. |
| `candidate_pnl` | `_candidate_pnl` | Parity | Handles single YES/NO and union sides. |
| `backtest_trade_row` | `_backtest_trade_row` | Near parity | Same key fields; rationale JSON formatting may differ. |
| `finalize` | `_finalize_backtest_run` | Near parity | Same headline metrics intent. Sharpe/day aggregation needs golden checks. |
| `diagnostic_counts` | `_diagnostic_counts` | Parity | Counts diagnostic reasons. |
| `auto_artifact_paths` | `_auto_artifact_paths` | Near parity | Same artifact set; stem differs. |
| `write_summary_csv` | `_write_summary_csv` | Partial | Flattens summary, but preferred column order is not byte-identical. |
| `write_trades_csv` | `_write_trades_csv` | Near parity | Same trade artifact purpose and sorting. |
| `write_summary_report` | `_write_summary_report` | Partial | Simplified report text. |
| `ensure_parent` | parent mkdir helpers | Parity | File-system helper. |
| `summary_to_flat_row` | `_flatten_csv_row` plus preferred fields | Partial | Rust does direct flattening of known fields. |
| `round4` / `round6` | Python `round` usage | Near parity | Same decimal intent. |

Python backtest gaps:

- `_seed_research_sources` not ported.
- `_hours_to_close_at` is not a standalone Rust helper; equivalent math is embedded.
- `_jsonish`, `_fmt_num`, `_fmt_pct`, `_turnover_yield`, `_diagnostics_text` are not standalone Rust helpers.
- Exact summary CSV/report formatting remains to golden-test and tighten.

## Calibration Analysis

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `CalibrationCell` | calibration cell dict | Rust-only | Typed cell helper. |
| `load` | `_load_calibration_table` | Parity | Loads calibration JSON. |
| `lead_bin` | `lead_bin` | Parity | Same lead-hour buckets. |
| `lookup` | `lookup` | Near parity | Cell min-sample, station default, global default chain. |
| `ErrorRow` | intermediate error row dict | Rust-only | Internal fit row. |
| `build_table` | `build_table` | Near parity | Joins forecasts/settlements and emits cells/defaults. Handles missing settlement-temp source columns on read-only DBs. |
| `fit` | `fit` | Near parity | Mean bias and clamped sample sigma. |
| `round4` | `round` usage | Parity | Output rounding. |

Python calibration gaps:

- Broader `_ensure_column` behavior is ported only for the columns Rust needs.
- `clamp` is inline via Rust `f64::clamp`.

## Bankroll Analysis

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `ReplayCandidate` | `ReplayCandidate` dataclass | Near parity | Typed replay input. Rust derives candidates from backtest trade rows. |
| `ReplayCandidate::from_trade_row` | conversion after `generate_replay_candidates` | Partial | Avoids a separate candidate generation loop by using backtest trade rows. |
| `OpenPosition` | `OpenPosition` dataclass | Near parity | Internal locked position. |
| `LedgerTrade` | trade dict in `apply_bankroll_ledger` | Near parity | Artifact row shape. |
| `LedgerSkip` | skip dict | Parity | Insufficient-cash skip shape. |
| `EquityPoint` | equity curve dict | Near parity | Same metrics and event types. |
| `LedgerSummary` | summary dict | Near parity | Same major metrics. |
| `LedgerResult` | result dict | Rust-only | Typed result wrapper. |
| `apply_bankroll_ledger` | `apply_bankroll_ledger` | Near parity | Cash, locked capital, scale-down, skip, settlement, drawdown, utilization. |
| `settle_due` | nested `settle_due` | Near parity | Releases stake plus PnL by resolved time. |
| `mark` / `mark_with_note` | nested `mark` | Near parity | Records equity curve and utilization. |
| `equity` | `_equity` | Parity | Cash plus locked capital. |
| `locked` | `_locked` | Parity | Sum allocated stake. |
| `round4` / `round6` / `round8` | Python `round` usage | Near parity | Decimal rounding helpers. |

Python bankroll gaps:

- `_fallback_resolved_at`, `_settlement_resolved_at`, and `_settlement_rows` are not standalone Rust helpers.
- `generate_replay_candidates` is not separately ported; Rust uses `run_backtest(include_trades=true)` as the candidate source.
- Full Python `run_bankroll_backtest` wrapper is split between `commands::bankroll::run` and `analysis::bankroll`.

## Sweep Analysis

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `analysis::sweep::SweepResult` | sweep result dict | Rust-only | Placeholder support type; command module owns current sweep behavior. |

Most sweep behavior currently lives in `commands::sweep` rather than
`analysis::sweep`.

## Data Models And Recorder Helpers

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `data::models::WeatherContract` | `data.models.WeatherContract` | Near parity | Typed normalized contract metadata. |
| `data::models::MarketSnapshot` | `data.models.MarketSnapshot` | Near parity | Typed market snapshot payload. |
| `data::models::MetarRecord` | `data.models.METARRecord` | Near parity | Typed METAR observation payload. |
| `data::models::ForecastRecord` | `data.models.ForecastRecord` | Near parity | Typed forecast payload. |
| `data::models::ForecastModelRun` | forecast model run dict | Rust-only/Near parity | Typed row for `forecast_model_runs`. |
| `data::gamma::ParsedMarket` | dict from `parse_markets` | Rust-only | Typed market parse result. |
| `data::gamma::build_slug` | `gamma_client.build_slug` | Parity | Delegates to configured daily-high slug builder. |
| `data::gamma::fetch_event` | `gamma_client.fetch_event` | Near parity | Fetches one event by city/date slug. |
| `data::gamma::fetch_event_by_slug` | `gamma_client.fetch_event_by_slug` | Near parity | Fetches one event by exact slug. |
| `data::gamma::fetch_events_range` | `gamma_client.fetch_events_range` | Near parity | Iterates daily slugs across a range. No sleep/rate-limit yet. |
| `data::gamma::parse_markets` | `gamma_client.parse_markets` | Near parity | Parses encoded `clobTokenIds` and `outcomePrices`. |
| `data::gamma::fetch_order_book` | `gamma_client.fetch_order_book` | Near parity | Fetches CLOB book. Python swallows errors; Rust returns them. |
| `data::gamma::fetch_fee_rate` | `gamma_client.fetch_fee_rate` | Near parity | Fetches and normalizes fee rate. Python swallows errors; Rust returns them. |
| `data::gamma::normalize_fee_rate` | `gamma_client.normalize_fee_rate` | Parity | Preserves `base_fee / 20_000` behavior. |
| `data::gamma::effective_ask_vwap` | `gamma_client.effective_ask_vwap` | Parity | Walks ask ladder and requires sufficient depth. |
| `data::gamma::book_depth` | `gamma_client.book_depth` | Parity | Sums level sizes. |
| `data::gamma::best_price` | `gamma_client.best_price` | Parity | Max bid/min ask. |
| `data::gamma::winning_bin` | `gamma_client.winning_bin` | Parity | YES outcome price >= 0.99. |
| `data::gamma::client` | `_get` session behavior | Rust-only | Builds blocking HTTP client. |
| `data::gamma::value_to_f64` / `value_to_string` | inline coercions | Rust-only | Shared JSON coercion helpers. |
| `data::gamma::parse_json_array_value` | `json.loads` fallback | Parity | Handles list or JSON-encoded string. |
| `data::gamma::abbreviated` | token log slicing | Rust-only | Internal helper for shorter error context. |
| `data::weather_contracts::WeatherContractDiscovery` | `WeatherContractDiscovery` | Near parity | Contract plus optional event. |
| `data::weather_contracts::WeatherContractDiscovery::is_valid` | `WeatherContractDiscovery.is_valid` property | Parity | Checks `validation_status == "valid"`. |
| `data::weather_contracts::parse_daily_high_slug` | `parse_daily_high_slug` | Parity | Slug to city/date. |
| `data::weather_contracts::configured_weather_contracts` | `configured_weather_contracts` | Parity | Generates allowlisted city/date contracts. |
| `data::weather_contracts::discover_configured_weather_contracts` | `discover_weather_contracts` | Partial | Fetches configured city/date slugs directly; does not page tag-filtered events. |
| `data::weather_contracts::validate_weather_event` | `validate_weather_event` | Near parity | Unit/source/station validation ported. |
| `data::weather_contracts::skipped` | `_skipped` | Near parity | Internal skipped-contract constructor. |
| `data::weather_contracts::contract_from_config` | `_contract` / `configured_weather_contracts` | Near parity | Internal configured contract constructor. |
| `data::weather_contracts::event_text` | `_event_text` | Parity | Concatenates event/market text. |
| `data::weather_contracts::detect_unit` | `_detect_unit` | Parity | Detects F/C wording. |
| `data::weather_contracts::source_url` | `_source_url` | Near parity | Uses `resolutionSource` or Wunderground URL regex. |
| `data::weather_contracts::station_from_source_url` | `_station_from_source_url` | Parity | Last URL segment uppercase. |
| `data::weather_contracts::fallback_target_date` | `_fallback_target_date` | Near parity | End date or slug date fallback. |
| `data::weather_contracts::raw_event_json` | raw JSON subset in `_contract` | Near parity | Captures title/slug/resolution source subset. |
| `data::weather_contracts::title_from_slug` | `_title_from_slug` | Parity | Title-cases slug words. |
| `data::weather_contracts::month_number` | `MONTHS` lookup | Parity | Month-name to number mapping. |
| `data::market_recorder::SnapshotRunSummary` | log/count summary | Rust-only | Structured summary for Rust command JSON. |
| `data::market_recorder::dates_to_record` | `_dates_to_record` | Parity | Today and tomorrow UTC. |
| `data::market_recorder::hours_to_close` | `_hours_to_close` | Parity | Hours until noon UTC. |
| `data::market_recorder::record_market_snapshots` | `record_market_snapshots` | Near parity | Fetches configured events, books, fees, and inserts snapshots. |
| `data::market_recorder::all_markets_open` | `_all_markets_open` | Parity | Any market not closed. |
| `data::metar_recorder::MetarRunSummary` | log summary | Rust-only | Structured summary for Rust command JSON. |
| `data::metar_recorder::c_to_f` | `metar_recorder.c_to_f` | Parity | Celsius to Fahrenheit rounded to one decimal. |
| `data::metar_recorder::fetch_metars` | `fetch_metars` | Near parity | Fetches Aviation Weather JSON for enabled stations. |
| `data::metar_recorder::parse_metar` | `parse_metar` | Near parity | Parses station, temp, dew point, obs time, raw METAR. |
| `data::metar_recorder::record_metars` | `record_metars` | Near parity | Fetches and inserts current METARs. |
| `data::metar_recorder::parse_obs_time` | inline obs-time parsing | Near parity | Handles epoch/string timestamps and ISO fallback. |
| `data::forecast_recorder::ForecastRunSummary` | log summary | Rust-only | Structured summary for Rust command JSON. |
| `data::forecast_recorder::c_to_f` | `forecast_recorder.c_to_f` | Parity | Celsius to Fahrenheit rounded to one decimal. |
| `data::forecast_recorder::f_to_c` | `forecast_recorder.f_to_c` | Parity | Fahrenheit to Celsius rounded to one decimal. |
| `data::forecast_recorder::lead_hours` | `_lead_hours` | Near parity | Uses `chrono-tz` for target-date local midnight. |
| `data::forecast_recorder::fetch_forecast` | `fetch_forecast` | Near parity | Fetches Open-Meteo ECMWF/GFS daily highs. |
| `data::forecast_recorder::record_forecasts` | `record_forecasts` | Near parity | Inserts forecast records and per-model rows. |

## Execution And Settlement Data Contracts

| Rust item | Python counterpart | Status | Notes |
|---|---|---:|---|
| `execution` module | execution helpers/scripts | Rust-only/empty | Reserved module; command code currently owns execution behavior. |
| `data` module | data package | Near parity | Gamma, contract validation, market snapshots, METARs, TAF/NWS, Open-Meteo forecasts, and backfill paths are ported. |

## Python-Only Data/Recorder Gaps

These Python functions/classes still differ materially from Rust:

- `data/gamma_client.py`: `_get` retry/backoff wrapper and `fetch_events_range`
  gentle rate limiting are not fully matched.
- `data/weather_contracts.py`: broad `fetch_weather_events` pagination and
  `discover_contracts_from_events` over arbitrary event lists are not fully ported.
- Backfill retry/backoff/rate-limit timing is simplified compared with Python.
- True forecast backfill is represented with `gfs-global-24h`-compatible
  metadata from historical Open-Meteo data rather than a byte-for-byte clone of
  the Python helper.

## Remaining Gaps By Area

This is the current open-work list compared with the original Python repo.

### Build And Verification

- `cargo fmt -- --check`, `cargo check`, `cargo test`, and
  `cargo run -- backtest --list-strategies` pass under the local WSL Rust
  toolchain installed at `/tmp/poly-weather-*`.
- Python-vs-Rust golden parity scripts now cover all requested parity surfaces,
  but real-DB end-to-end parity still needs to be run on selected date ranges.
- Fixture integration tests synthesize a Python-compatible SQLite DB at runtime
  and cover backtest, bankroll, calibration shape, and paper execute/settle
  lifecycle.

### Backtest And Artifacts

- Summary CSV flattening/order is close but not byte-identical to
  `_write_summary_csv`.
- Markdown report text is simplified versus `_write_summary_report`.
- `_seed_research_sources` is not ported.
- `_jsonish`, `_fmt_num`, `_fmt_pct`, `_turnover_yield`, and
  `_diagnostics_text` are not separate Rust helpers.
- Real DB golden checks are still needed for forecast ordering, snapshot fills,
  Sharpe/daily aggregation, trade rows, and diagnostics.

### Bankroll

- Rust derives bankroll candidates from Rust backtest trade rows instead of
  separately porting `generate_replay_candidates`.
- `_settlement_rows`, `_settlement_resolved_at`, and `_fallback_resolved_at`
  are not separate Rust helpers.
- Bankroll artifact field order/text still needs golden comparison against
  Python.

### Parameter Sweep

- `--jobs` is implemented with Rust threads rather than Python worker
  processes; exact scheduling/log output differs.
- Python's `exhaustive` preset is implemented, but exact grid values still need
  golden comparison.
- `available_dates` parity is incomplete: Rust uses min/max settlement range,
  while Python filters each date for forecast and price-history availability.
- Sweep CSV/report column order and compact parameter formatting still need
  golden checks.

### Calibration

- Dynamic missing-column compatibility for settlement temp source fields is
  ported for writable DBs and read-only calibration queries.
- `print_table`/`calibrate --show` is ported.
- Fitting output shape is implemented, but should still be golden-compared on
  the same DB.

### Execution And Settlement

- Live order submission from `execute.py::submit_trade` is intentionally not
  ported and must continue to fail loudly.
- `execute.py` logging setup is not ported.
- Settlement source enrichment is improved: Rust inserts Gamma-derived
  settlement rows, source/station/raw summary metadata, and paper closes
  positions, but does not fully port
  `settlement_tracker._extract_settlement` metadata/source handling.
- Settlement edge-bin midpoint behavior is approximate and needs fixture parity.

### Recorder And Data Collection

- Continuous scheduling from `run_recorder.py` is ported with
  `--interval-seconds`.
- TAF recording is ported.
- NWS forecast recording is ported.
- ASOS daily high settlement-temperature backfill is ported.
- ASOS METAR historical backfill is ported.
- Historical Open-Meteo forecast backfill is ported.
- True 24h GFS-style forecast backfill is ported using the historical forecast
  source and `gfs-global-24h`-compatible model-run metadata.
- CLOB price-history backfill is ported.
- Missing-bin price-history retry backfill dispatches through the price-history
  backfill path.
- Broad weather event pagination/discovery over arbitrary Gamma event lists is
  partial; Rust currently fetches configured slugs directly.
- Gamma `_get` retry/backoff and gentle rate limiting are not fully matched.

### DB Layer

- Dynamic settlement temp source columns are ensured for writable DBs; broader
  auxiliary migrations still rely mostly on the copied schema.
- Insert/query helpers for TAF, NWS, ASOS-derived METARs, price history, true
  forecasts, and historical forecasts are implemented.

### Minor API/Formatting Differences

- `status.py::fmt` formatting is not ported exactly; Rust prints a similar table.
- `contract_for_display_city` is ported as a standalone helper.
- `list_strategy_names` is implemented alongside `list_strategy_configs` and
  group helpers.
- Unknown configured strategy handling is stricter in the registry; low-level
  plugin fallback still maps unknown plugin IDs to unsupported diagnostics.

## Current High-Risk Differences To Golden Test

1. Backtest summary CSV flattening and Markdown report text.
2. Forecast snapshot ordering on real mixed-source rows.
3. Snapshot fill VWAP selection when order book ladders are sparse.
4. Bankroll candidate derivation from trade rows versus Python's separate
   `generate_replay_candidates` path.
5. Parameter sweep output column ordering and final report text.
6. Settlement source enrichment and edge-bin midpoint behavior.
7. Live market snapshot recorder behavior around closed events, sparse books,
   fee-rate failures, and configured-slug discovery versus broad event search.
8. Live METAR/Open-Meteo recorder behavior around API field changes, duplicate
   rows, and local-time lead-hour parity.

## Verification State

- `cargo fmt -- --check` passes.
- `cargo check` passes with local WSL Rust:
  `CARGO_HOME=/tmp/poly-weather-cargo RUSTUP_HOME=/tmp/poly-weather-rustup /tmp/poly-weather-cargo/bin/cargo check`.
- `cargo test` passes with the same WSL Rust toolchain: 38 tests total.
- `cargo run -- backtest --list-strategies` passes with the same WSL Rust
  toolchain.
- `scripts/parity_compare.py --check backtest` passes for NYC `2026-04-01` to
  `2026-04-02` against `../poly-weather/db/data.db`.
- Windows MSVC Cargo remains blocked by missing VC C headers for bundled SQLite;
  the working local execution path is WSL Rust.
- No files in `../poly-weather` were modified.
