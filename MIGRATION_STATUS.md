# Migration Status

## Stage 1: Scaffold And Static Compatibility

Done.

- New standalone crate: `poly-weather-rust`.
- Compatible assets copied from Python repo:
  - `db/schema.sql`
  - `analysis/strategies/strategies.json`
  - `calibration.json`
- CLI commands are present for all planned Python surfaces.
- Mutating commands default to `db/data.db` inside this Rust repo unless `--db`
  is explicitly provided.

## Stage 2: Backtest Parity First

Implemented for the validated core replay surfaces.

- Strategy core and executable plugins are ported.
- Calibration lookup is ported.
- SQLite read queries for settlements, forecasts, minute price history, market
  snapshot fills, METARs, climatology, and positions are ported.
- `gfs-global-24h` model-run availability parsing is ported.
- Forecast selection now sorts by source priority, exact lead-distance, then
  recency.
- Snapshot VWAP fills are used when available, with price-history proxy fallback.
- Interval backtest runner writes summary CSV, trades CSV, Markdown report, and
  final JSON stdout.
- Python-vs-Rust parity harness exists at `scripts/parity_compare.py` and now
  covers backtest, strategy group, bankroll, quick sweep, calibration shape, and
  paper execute lifecycle.
- Backtests now use a shared preloaded replay dataset so strategy groups and
  sweeps do not repeatedly reload the same SQLite rows.
- Broad all-city replays use a hybrid loader that preserves compatibility with
  the read-only Python DB indexes while still sharing the loaded replay data.
- `max_stake_usd: null` now matches Python's unlimited daily-cap behavior.

Known remaining backtest gaps:

- Summary CSV/report text is close but not guaranteed byte-identical to Python.
- Research-source seeding remains intentionally unported.

## Stage 3: Bankroll, Sweep, Calibration

Implemented for the validated parity surfaces.

- Bankroll replay now uses Python-compatible replay candidate generation rather
  than accepted turnover-backtest trades, then models cash, locked capital,
  settlement release, scale-down or skips, equity curve, summary JSON/CSV,
  trades CSV, skips CSV, equity CSV, and Markdown report.
- Parameter sweep now expands quick/full/exhaustive presets, common grids,
  strategy-specific grids, `--param-grid`, `--strategy-param-grid`, parallel
  `--jobs`, ranking with `--min-trades`, per-strategy artifacts, and
  best-by-strategy report. Sweep workers share the immutable replay dataset.
- Calibration fitting from `forecast_records` plus `settlement_records` is
  implemented with station/global fallbacks, dynamic missing-column read
  compatibility, `--show`, and JSON output.

Known remaining analysis gaps:

- Some Markdown wording remains simplified relative to Python.

## Stage 4+: Data, Paper Execution, Settlement

Implemented with remaining parity risks.

- `execute` parses `execution_plan.json`, rejects live execution, records
  dry-run/paper positions, and exports `open_positions.json`.
- `settle` can fetch Gamma event outcomes by slug, insert settlement records,
  close paper positions, and export `open_positions.json`.
- Gamma/CLOB read helpers are ported for event lookup, market parsing, order
  books, fee rates, VWAP ask fills, book depth, best prices, and winning-bin
  detection.
- Weather contract slug parsing, configured-contract materialization, and
  Wunderground station validation are ported.
- `recorder --once` now records METAR observations, TAFs, NWS forecasts, market
  snapshots, and Open-Meteo forecasts/model runs into the Rust DB.
- Continuous recorder scheduling is implemented with `--interval-seconds`.
- Recorder/backfill flags now dispatch to Rust implementations for ASOS daily
  highs, ASOS METARs, historical Open-Meteo forecasts, true 24h GFS-style
  forecasts, CLOB price history, and missing-bin retries.
- DB insert/query helpers now cover TAF, NWS, price history, historical
  forecasts, true forecasts, and settlement temp source metadata.
- Live order submission is intentionally not ported and returns:
  `unsupported: live order submission is intentionally not ported`.

Known remaining data/settlement risks:

- Broad Gamma weather-event pagination/discovery remains partial; Rust mainly
  fetches configured slugs directly.
- Gamma retry/backoff and rate-limit timing are not byte-for-byte Python parity.
- Settlement source enrichment is improved but not a full clone of Python's
  historical `settlement_tracker` metadata extraction.

## Verification

- `cargo check` passes.
- `cargo test` passes: 35 unit tests plus 4 integration tests.
- `cargo run -- backtest --list-strategies` passes.
- `scripts/parity_compare.py --check all` passes for `2026-04-01` through
  `2026-04-02` against `../poly-weather/db/data.db`.
- Full all-city April no-cap `base_no_ev` Rust release run completes in about
  92 seconds and matches the Python headline metrics:
  `total_trades=1050`, `wins=812`, `win_rate=0.7733`,
  `total_pnl=566.6529`, `total_stake=10270.0`,
  `turnover_yield=0.055176`, `pnl_per_active_day=19.5398`,
  `days_considered=319`, `interval_bars_considered=684187`.
- Windows MSVC Cargo remains environment-blocked because Visual Studio is
  missing VC C headers for bundled SQLite; WSL Rust is the working path here.
