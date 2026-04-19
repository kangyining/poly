# Polymarket Weather Trading Bot

An automated trading system that bets on Polymarket daily high-temperature markets using calibrated NWS forecasts and fee-adjusted expected value scoring.

---

## What It Does

Polymarket lists daily markets asking "Will the highest temperature in NYC be between X–Y°F today?" across 11 temperature bins per city. This bot:

1. Fetches the live NWS forecast for the target city
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
EV_NO = (1 − P_model) × (1 − fee_rate × P_model) − effective_ask
```
A bin qualifies when `EV_NO ≥ min_ev_threshold` (default 0.02).

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
│    .json every 5 min        │     │  • settle.py   ←── daily     │
└─────────────────────────────┘     └──────────────────────────────┘
```

**Data flow per 5-minute pass:**
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
- **Fee-adjusted EV** — accounts for Polymarket taker fee (10% bps) on NO side
- **5-minute polling loop** — configurable via `--poll-interval-secs`
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

## Configuration

All flags can be set via CLI argument or environment variable.

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--run-loop` | `LOOP` | false | Continuous loop (vs single pass) |
| `--dry-run` | `DRY_RUN` | **true** | Paper mode — no real orders |
| `--poll-interval-secs` | `POLL_INTERVAL_SECS` | 300 | Seconds between passes |
| `--min-ev-threshold` | `MIN_EV_THRESHOLD` | 0.02 | Minimum EV to qualify a bin |
| `--desired-shares` | `DESIRED_SHARES` | 10.0 | Shares to buy per bin |
| `--order-size-usd` | `ORDER_SIZE_USD` | 5.0 | Dollar stake per bin |
| `--max-total-stake-usd` | `MAX_TOTAL_STAKE_USD` | 50.0 | Max stake per run |
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
| `poly-weather-bot/target/release/polymarket-weather-bot.exe` | Compiled bot binary |

---

## Project Structure

```
Polymark/
├── README.md
├── script_desc.md
├── plan.md               ← full build plan (Phase 1–4)
├── strategies.md         ← strategy guide
├── phase.md              ← phase gate criteria
│
├── poly-weather/         ← Python data pipeline
│   ├── execute.py        ← order submission bridge
│   ├── settle.py         ← daily settlement runner
│   ├── status.py         ← position and PnL viewer
│   ├── calibrate.py      ← calibration re-fitter
│   ├── run_recorder.py   ← market snapshot recorder
│   ├── analysis/
│   │   └── calibration.py
│   ├── config/
│   │   ├── cities.py
│   │   └── params.py
│   ├── data/
│   │   ├── db.py
│   │   ├── models.py
│   │   ├── settlement_tracker.py
│   │   └── ...
│   └── db/
│       ├── schema.sql
│       └── data.db
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
