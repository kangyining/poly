# Poly

A Polymarket weather research and trading monorepo, combining four projects from
`lynx-canadensis` with contributor attribution and sanitized Git history.

| Project | Purpose |
| --- | --- |
| [poly-weather](poly-weather/) | Python data collection, SQLite storage, calibration and backtesting. |
| [poly-weather-rust](poly-weather-rust/) | Rust migration of the research backend; paper execution only. |
| [poly-weather-bot](poly-weather-bot/) | Event-driven Rust strategy/execution engine; dry run by default. |
| [poly-dashboard](poly-dashboard/) | FastAPI API and React dashboard. |

## Development checks

From the repository root, with Python 3.14+, Node 24 and a Rust toolchain:

```sh
python -m pip install -r poly-weather/requirements.txt
python -m pip install -r poly-dashboard/apps/api/requirements-dev.txt
python poly-weather/backtest.py --list-strategies
npm --prefix poly-dashboard/apps/web ci --ignore-scripts
npm --prefix poly-dashboard/apps/web run build
python -m unittest discover -s poly-dashboard/apps/api/tests
cargo test --locked --manifest-path poly-weather-rust/Cargo.toml
cargo check --locked --all-targets --manifest-path poly-weather-bot/Cargo.toml
python tools/verify_import.py
```

Rust dependencies need a native C toolchain. The [CI workflow](.github/workflows/review-checks.yml)
also runs the Python fixture suite. Each component keeps its own manifest and lockfile.

## Current limits

- Dashboard backtests use the Python strategy registry and 1-minute replay, with structured JSON results and file-based history.
- The bot writes JSON positions; the dashboard reads Python SQLite state.
- Runtime databases, document caches, logs and portfolio files have been removed from shared history. Generate synthetic inputs with `python tools/review_demo.py --output tmp/review-demo` and point `POLY_WEATHER_DB_PATH` at its `data.db`.

Passing checks do not establish profitable trading or complete dashboard integration.

Sanitized source branch heads remain under `archive/<repo>/<branch>`.
The verifier checks the immutable import snapshot, the sanitized source commits and 12 archived
branches recorded in [the source manifest](docs/source-repositories.json).

The history cleanup changes commit IDs; full originals are backed up locally.
