# Reviewer guide

This is a collaborative Polymarket weather-system codebase imported from four
`lynx-canadensis` repositories. The source history and contributor attribution are
preserved. The directory structure describes a system in transition, including a
Rust migration of the Python backend alongside the separate Rust trading bot.

## Useful review paths

| Focus | Start here | Check |
| --- | --- | --- |
| Research logic | `poly-weather/analysis/strategies/` | Probability/fee math, strategy contracts, explicit missing-data diagnostics and fixtures. |
| Backend migration | `poly-weather-rust/src/` and `scripts/parity_compare.py` | Typed models, SQLite compatibility, replay implementation and limits of the recorded parity evidence. |
| Event-driven engine | `poly-weather-bot/src/app.rs`, `core/`, `consumers/`, `services/` | Event fan-out, shared state, evaluation triggers and the execution boundary. |
| Frontend | `poly-dashboard/apps/web/` | React data-fetching contracts and live/backtest views. |

The strategy score and historical research outputs are not evidence of profitable
live trading. The data windows, model calibration and execution assumptions need
their own research validation.

## Reproduce the checks

Python fixtures, from `poly-weather/`:

```sh
python -m pip install -r requirements.txt
python -m unittest analysis.strategies.test_strategies data.test_weather_contracts data.test_market_recorder data.test_us_high_temp_data_paths analysis.test_bankroll_backtest
```

Frontend, from `poly-dashboard/apps/web/`, with Node 24:

```sh
npm ci --ignore-scripts
npm run build
```

The lockfile includes the previously missing optional WASM dependency records;
the clean install and production build now pass on the review machine. Vite
reports a large-chunk advisory, which is a bundle-size improvement rather than a
failed build.

Rust fixture checks, from the repository root:

```sh
cargo test --locked --manifest-path poly-weather-rust/Cargo.toml
cargo check --locked --all-targets --manifest-path poly-weather-bot/Cargo.toml
```

Local Windows compilation is blocked by missing C headers in the installed
MSVC/Windows SDK setup (`stdarg.h` for bundled SQLite, plus native TLS/crypto build
requirements). The added Linux CI job provides a separate reproducible check;
its result should be assessed independently of the source's older test notes.

## Current integration limits

The backend CLI and strategy fixtures are the strongest current demonstration
path. The dashboard API still passes obsolete replay flags, expects a legacy
strategy file/output shape and reads Python SQLite positions while the current
bot writes JSON state. See [the assessment](ASSESSMENT.md) for exact evidence.
The lockfile repair does not resolve these API/data-contract mismatches.

Previously tracked runtime database/log/portfolio artifacts remain in the import
and its history. New ignore rules prevent additional matching artifacts from
being added by default; they do not remove existing data. Any external review
should use the intended repository scope and project-contribution context.

`python tools/verify_import.py` checks the historical import commit recorded in
the manifest and all archived source heads, so ordinary development changes do
not invalidate the import evidence.
