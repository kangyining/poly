# poly-weather-rust

Standalone Rust migration of the Python `poly-weather` repo.

This repo is intentionally created next to the Python repo and does not modify
`../poly-weather`.

## Current Migration Status

Implemented:

- Cargo crate and CLI shell with Python-compatible command groups.
- Static weather contract config.
- Copied compatibility assets:
  - `db/schema.sql`
  - `analysis/strategies/strategies.json`
  - `calibration.json`
- SQLite read layer for backtest/status parity.
- Strategy core and executable strategy plugins:
  - `base_no_ev`
  - `base_yes_no_ev`
  - `ensemble_bucket_ev`
  - `dynamic_sigma_ev`
  - `bayesian_climatology_ev`
  - `corridor_ladder`
  - `threshold_union`
  - `basket_arb`
  - `maker_quote_sim`
  - `nowcast_metar`
- Research strategy placeholders with explicit missing-data diagnostics.
- Read-only interval backtest runner with CSV/Markdown/JSON artifact output.
- Read-only position status viewer.
- Bankroll replay with locked capital, scale-down/skips, equity curve, and
  CSV/JSON/Markdown artifacts.
- Parameter sweep with quick/full/exhaustive grids, custom grids, `--jobs`
  parallel workers, ranking, per-strategy artifacts, and best-by-strategy report.
- Calibration fitting from forecast/settlement rows, read-only compatibility for
  older schemas missing settlement temp source columns, and `--show`.
- Dry-run/paper `execute` with `execution_plan.json` parsing and
  `open_positions.json` export.
- Basic `settle` flow for Gamma outcome lookup, settlement inserts, paper
  position closeout, and `open_positions.json` export.
- Gamma/CLOB data helpers for event lookup, market parsing, order books, fee
  rates, VWAP fills, depth, best prices, and winning-bin detection.
- Weather-contract slug parsing, configured contract generation, and
  Wunderground station validation.
- `recorder --once` live METAR, TAF, NWS, market snapshot, and Open-Meteo
  forecast recording into the Rust DB.
- Continuous recorder scheduling via `recorder --interval-seconds`.
- Rust backfill command paths for ASOS daily highs, ASOS METARs, historical
  Open-Meteo forecasts, true 24h GFS-style forecasts, CLOB price history, and
  missing-bin price retries.
- Dynamic DB compatibility columns for settlement temp source metadata.
- Expanded parity harness and fixture integration tests for backtest, bankroll,
  calibration, and paper execute/settle lifecycle.
- Unit and integration tests for strategy math/config/backtest/data helpers.

Still not claimed byte-identical:

- Markdown/CSV wording and ordering in some reports.
- Broad Gamma weather-event pagination/discovery and exact retry/backoff timing.
- Full settlement-source metadata parity with Python's historical tracker.

Live order submission is intentionally excluded. `execute` fails loudly with
`unsupported: live order submission is intentionally not ported` when live mode
is requested.

## Build

```bash
CARGO_HOME=/tmp/poly-weather-cargo \
RUSTUP_HOME=/tmp/poly-weather-rustup \
/tmp/poly-weather-cargo/bin/cargo check
```

For this implementation pass, a local WSL Rust toolchain was installed under
`/tmp/poly-weather-*` and validated with bundled SQLite plus Rustls TLS. The
Windows MSVC Cargo path is still not reliable on this machine because Visual
Studio is missing the VC C headers needed to compile bundled SQLite.

## Backtest Parity Smoke

```bash
cargo run -- backtest \
  --db ../poly-weather/db/data.db \
  --strategy base_no_ev \
  --city NYC \
  --date-from 2026-04-01 \
  --date-to 2026-04-05
```

The command reads the Python SQLite DB in read-only mode and writes artifacts
under this repo's `backtest/` directory.

## Recorder Snapshot Smoke

```bash
cargo run -- recorder --once
```

This records live METARs, configured weather contracts, live market snapshots,
TAFs, NWS forecasts, Open-Meteo forecasts, and forecast model rows into this
repo's default `db/data.db`.

## Parity Harness

```bash
python3 scripts/parity_compare.py \
  --db ../poly-weather/db/data.db \
  --city NYC \
  --date-from 2026-04-01 \
  --date-to 2026-04-05 \
  --check all
```

The harness runs Python and Rust checks against the same DB for backtest,
strategy-group backtest, bankroll, quick sweep, calibration shape, and paper
execute lifecycle. It exits nonzero on mismatches.
