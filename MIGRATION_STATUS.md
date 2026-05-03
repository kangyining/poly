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

Mostly implemented, still needs real-DB golden parity runs.

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

Known remaining backtest gaps:

- Summary CSV/report text is close but not guaranteed byte-identical to Python.
- Golden parity needs to run once linked test/binary execution is available.

## Stage 3: Bankroll, Sweep, Calibration

Mostly implemented.

- Bankroll replay now models cash, locked capital, settlement release, scale-down
  or skips, equity curve, summary JSON/CSV, trades CSV, skips CSV, equity CSV,
  and Markdown report.
- Parameter sweep now expands quick/full/exhaustive presets, common grids,
  strategy-specific grids, `--param-grid`, `--strategy-param-grid`, parallel
  `--jobs`, ranking with `--min-trades`, per-strategy artifacts, and
  best-by-strategy report.
- Calibration fitting from `forecast_records` plus `settlement_records` is
  implemented with station/global fallbacks, dynamic missing-column read
  compatibility, `--show`, and JSON output.

Known remaining analysis gaps:

- Bankroll candidate rows are derived from Rust backtest trade rows, so any
  remaining backtest artifact mismatch propagates into bankroll.
- Some artifact wording/ordering still needs real Python golden comparison.

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

- `cargo fmt -- --check` passes.
- `cargo check` passes with local WSL Rust:
  `CARGO_HOME=/tmp/poly-weather-cargo RUSTUP_HOME=/tmp/poly-weather-rustup /tmp/poly-weather-cargo/bin/cargo check`.
- `cargo test` passes with the same WSL Rust toolchain: 38 tests total.
- `cargo run -- backtest --list-strategies` passes with the same WSL Rust
  toolchain.
- Python-vs-Rust backtest parity passes for NYC `2026-04-01` through
  `2026-04-02` using `scripts/parity_compare.py --check backtest`.
- Windows MSVC Cargo remains environment-blocked because Visual Studio is
  missing VC C headers for bundled SQLite; WSL Rust is the working path here.
