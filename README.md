# Polymarket Weather Trading Bot

An automated trading system that bets on Polymarket daily high-temperature markets using calibrated NWS forecasts and fee-adjusted expected value scoring.

---

## What It Does

Polymarket lists daily markets asking "Will the highest temperature in NYC be between X–Y°F today?" across 11 temperature bins per city. This bot:

1. Refreshes the live NWS forecast for the target city on its own cadence
2. Applies a calibration bias correction (`beta_f`, `sigma_f`) fitted from historical forecast vs. settlement data
3. Computes the model probability that each bin wins using a Gaussian CDF
4. Fetches live orderbook depth and fee rate from the Polymarket CLOB
5. Scores each bin with fee-adjusted expected value on the **NO side**
6. Submits GTC limit orders on qualifying bins (or records paper positions in dry-run mode)
7. Tracks positions in SQLite; settles with PnL the morning after

---

## Strategy

**NO-first**: buy NO tokens on bins that are far from the forecast. If NWS says 57°F, a bin of "48–49°F" has ~1% chance of winning — the NO token pays $1 if (as expected) that bin loses. The edge comes when the market underprices that probability relative to the model.

**EV formula:**
```
EV_NO = (1 − P_model) − effective_ask × (1 + fee_rate × (1 − effective_ask))
```
A bin qualifies when `EV_NO ≥ min_ev_threshold` (live default 0.10).

**Calibration:** Raw NWS forecasts have a warm bias (+1.35°F at KLGA historically). The calibration table corrects for this per station, month, and forecast lead time.

---

## Architecture

```
┌─────────────────────────────┐     ┌──────────────────────────────┐
│   poly-weather-bot  (Rust)  │     │    poly-weather  (Python)    │
│                             │     │                              │
│  • NWS forecast fetch       │     │  • SQLite database           │
│  • Calibration lookup       │     │  • Settlement tracking       │
│  • CLOB orderbook walk      │     │  • Position management       │
│  • EV scoring               │     │  • Calibration fitting       │
│  • Risk limits              │     │  • execute.py  ←── orders    │
│  • Writes execution_plan    │────▶│  • status.py   ←── PnL view  │
│    .json every 15 sec       │     │  • settle.py   ←── daily     │
└─────────────────────────────┘     └──────────────────────────────┘
```

**Data flow per evaluation pass:**
```
NWS API → forecast → calibration → CLOB orderbook → EV score
    → execution_plan.json → execute.py → positions DB → open_positions.json
```

**Settlement flow (next morning):**
```
settle.py → Polymarket Gamma API → winning bin → close positions → PnL in DB
```

---

## Features

- **Live NWS forecast** via `api.weather.gov` — no paid data subscription needed
- **Calibrated Gaussian model** — bias-corrected mu and sigma per station, month, lead time
- **CLOB orderbook walk** — VWAP effective ask over desired shares; never uses Gamma mid-price
- **Fee-adjusted EV** — uses the CLOB fee-rate endpoint instead of assuming a fixed book cost
- **Split refresh loop** — 15-second market repricing plus 30-minute weather refresh by default
- **Paper trading (default)** — full dry-run mode; positions tracked identically to live
- **Position deduplication** — skips bins already held; no double-buying
- **Risk limits** — max total stake per run and max open positions
- **Auto market switching** — advances to next day's market after noon UTC
- **Full PnL tracking** — SQLite positions table; settled by `settle.py` each morning

---

## Supported Markets

| City | Station | Coordinates |
|------|---------|-------------|
| NYC  | KLGA (LaGuardia) | 40.7769, -73.874 |
| Chicago | KORD (O'Hare) | 41.88, -87.63 |
| Dallas | KDAL (Love Field) | 32.84, -96.85 |

> Currently runs NYC only. Chicago and Dallas are in the calibration table but not yet wired to the bot loop (Phase 4.4).

---

## Prerequisites

**Rust bot:**
```powershell
# Install Rust (https://rustup.rs)
cd poly-weather-bot
cargo build --release
```

**Python bridge:**
```powershell
cd poly-weather
pip install requests schedule
# For live trading: pip install py-clob-client==0.34.6
```

**Calibration (one-time, then weekly):**
```powershell
cd poly-weather
pip install pandas numpy scipy
python calibrate.py
```

---

## Quick Start (Paper Trading)

```powershell
# 1. Generate calibration
cd F:\Polymark\poly-weather
python calibrate.py

# 2. Start the bot (paper mode is the default)
cd F:\Polymark\poly-weather-bot
run_bot.bat --run-loop

# 3. Check positions any time
cd F:\Polymark\poly-weather
python status.py

# 4. Next morning: settle and see PnL
python settle.py
```

---

## Dashboard

The current dashboard surface lives in `poly-dashboard/`.

```powershell
# API
cd F:\Polymark\poly-dashboard
python -m uvicorn --app-dir apps/api main:app --reload --host 127.0.0.1 --port 8000

# Web
npm run dev:web
```

---

## Backtesting

Use `poly-weather/backtest.py` as the only supported backtest entry point.
It delegates to `analysis/phase1_backtest.py`, reads historical data from
SQLite, and writes result artifacts to CSV/Markdown files without inserting
simulated results into `backtests` or `positions`.
By default, backtests use 1-minute interval replay.

```powershell
cd F:\Polymark\poly-weather
python backtest.py --strategy base_ev --days 60 --city NYC
python backtest.py --strategy aggressive_tail --date-from 2026-02-01 --date-to 2026-04-18 --city NYC
```

The dashboard backtesting screen calls this same command path through the API.

### Strategy Lab

The Python backtester now uses pluggable strategies from
`analysis/strategies/strategies.json`. `base_ev` is kept as a compatibility
alias for `base_no_ev`; `poly-weather-bot` is not changed by this lab layer.

```powershell
python backtest.py --list-strategies
python backtest.py --strategy base_no_ev --days 60 --city NYC
python backtest.py --strategy-group core --date-from 2026-04-01 --date-to 2026-04-05 --city NYC
python backtest.py --all-strategies --date-from 2026-04-01 --date-to 2026-04-25 --city NYC
python backtest.py --strategy nowcast_metar --days 7 --city NYC
```

Every backtest run writes a summary CSV, trade-level CSV, and Markdown
comparison report under `backtest/`. Pass `--csv`, `--trades-csv`, or
`--report` only when you want custom artifact paths.

Implemented strategy plugins include exact-bucket NO EV, YES/NO EV,
ensemble-style bucket EV, dynamic sigma EV, climatology blending, corridor
ladders, synthetic threshold unions, basket arbitrage scans, maker quote
simulation, and METAR nowcasting. Research-only candidates such as monthly
remainder, semantic arbitrage, exit overlays, wallet-copy signals, and
resolution-lag monitoring emit explicit missing-data diagnostics until their
required data is available.

### Parameter Sweeps

Use `backtest_param_finetune/param_sweep.py` to test combinations of the
strategy lab plugin parameters. It reuses `analysis/phase1_backtest.py`, so
each combination uses the same engine and writes ranked CSV/Markdown summaries
under `backtest_param_finetune/results/`.
Default sizing sweeps `desired_shares` from `1` through `10`, does not use
`funding_pct`, and has no daily/global stake cap unless you pass
`--daily-max-stake-usd`.

```powershell
cd F:\Polymark\poly-weather

python backtest_param_finetune\param_sweep.py `
  --backtest-ready-strategies `
  --sweep-name nyc_april_all_strategies_uncapped_v1 `
  --city NYC `
  --date-from 2026-04-01 `
  --date-to 2026-04-25 `
  --rank-by roi `
  --min-trades 5
```

That sweeps all currently executable strategy plugins using the built-in quick
grid, writes one report per strategy, and writes a final best-by-strategy
comparison. For a custom range/grid:

```powershell
python backtest_param_finetune\param_sweep.py `
  --strategy dynamic_sigma_ev `
  --strategy maker_quote_sim `
  --sweep-name nyc_april_dynamic_maker_uncapped_v1 `
  --city NYC `
  --date-from 2026-04-01 `
  --date-to 2026-04-25 `
  --min-ev-thresholds 0.01:0.10:0.01 `
  --desired-shares 1:10:1 `
  --strategy-param-grid dynamic_sigma_ev:dynamic_sigma_min=0.4:1.2:0.2 `
  --strategy-param-grid maker_quote_sim:tick_size=0.005,0.01,0.02 `
  --rank-by roi `
  --min-trades 5
```

Use a train/validation split when choosing final values; for example tune on
February-March and validate on April rather than ranking and reporting on the
same period.

---

## Configuration

All flags can be set via CLI argument or environment variable.

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--run-loop` | `LOOP` | false | Continuous loop (vs single pass) |
| `--dry-run` | `DRY_RUN` | **true** | Paper mode — no real orders |
| `--poll-interval-secs` | `POLL_INTERVAL_SECS` | 15 | Legacy market polling knob; current live loop is websocket/event-driven |
| `--forecast-refresh-secs` | `FORECAST_REFRESH_SECS` | 1800 | Seconds between forecast refreshes |
| `--min-ev-threshold` | `MIN_EV_THRESHOLD` | 0.10 | Minimum EV to qualify a bin |
| `--desired-shares` | `DESIRED_SHARES` | 5.0 | Shares to buy per bin |
| `--order-size-usd` | `ORDER_SIZE_USD` | 5.0 | Dollar stake per bin |
| `--max-total-stake-usd` | `MAX_TOTAL_STAKE_USD` | 5.0 | Max stake per run |
| `--max-open-positions` | `MAX_OPEN_POSITIONS` | 8 | Max concurrent open positions |
| `--city` | `CITY` | nyc | City slug for event lookup |
| `--station-code` | `STATION_CODE` | KLGA | Station for calibration lookup |
| `--forecast-high-f` | `FORECAST_HIGH_F` | — | Manual override (skips NWS) |

**Live trading** additionally requires:
```powershell
set POLYMARKET_PRIVATE_KEY=0x...
set POLYMARKET_FUNDER=0x...
run_bot.bat --run-loop --dry-run=false
```

---

## Key Files

| File | Description |
|------|-------------|
| `poly-weather/db/data.db` | SQLite database (positions, settlements, forecasts, snapshots) |
| `poly-weather/calibration.json` | Calibration table (regenerated weekly by `calibrate.py`) |
| `poly-weather/execution_plan.json` | Written by bot each pass; read by `execute.py` |
| `poly-weather/open_positions.json` | Current open positions; read by bot to skip held bins |
| `poly-weather/backtest.py` | Supported CLI entry point for historical backtests |
| `poly-weather/analysis/phase1_backtest.py` | Current backtest engine used by the CLI and dashboard |
| `poly-weather/backtest_param_finetune/param_sweep.py` | Parameter grid-search runner for backtests |
| `poly-weather-bot/target/release/polymarket-weather-bot.exe` | Compiled bot binary |

---

## Project Structure

```
Polymark/
├── plan.md               ← full build plan (Phase 1–4)
├── strategies.md         ← strategy guide
├── phase.md              ← phase gate criteria
│
├── poly-weather/         ← Python data pipeline
│   ├── execute.py        ← order submission bridge
│   ├── settle.py         ← daily settlement runner
│   ├── status.py         ← position and PnL viewer
│   ├── calibrate.py      ← calibration re-fitter
│   ├── backtest.py       ← supported backtest entry point
│   ├── backtest_param_finetune/
│   │   ├── param_sweep.py
│   │   └── README.md
│   ├── run_recorder.py   ← market snapshot recorder
│   ├── analysis/
│   │   ├── calibration.py
│   │   └── phase1_backtest.py
│   ├── config/
│   │   ├── cities.py
│   │   └── params.py
│   ├── data/
│   │   ├── db.py
│   │   ├── models.py
│   │   ├── settlement_tracker.py
│   │   └── ...
│   ├── script_desc.md
│   ├── weather-bot-design.md
│   └── db/
│       ├── schema.sql
│       └── data.db
│
├── poly-dashboard/       ← current API + web dashboard
│   └── apps/
│       ├── api/
│       └── web/
│
└── poly-weather-bot/     ← Rust trading engine
    ├── run_bot.bat
    ├── Cargo.toml
    └── src/
        ├── main.rs
        ├── config.rs
        ├── polymarket.rs
        ├── weather.rs
        ├── calibration.rs
        ├── execution.rs
        ├── logging.rs
        └── strategy/
            ├── mod.rs
            ├── engine.rs
            ├── rules.rs
            └── types.rs
```
