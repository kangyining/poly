# Script Reference

All scripts live in `poly-weather/` (Python) or `poly-weather-bot/` (Rust/batch) unless noted.

---

## Daily Operation

### `run_bot.bat` — Launch the trading bot
**Location:** `poly-weather-bot/run_bot.bat`  
**When:** Start of day (or leave running continuously)

```powershell
cd F:\Polymark\poly-weather-bot

run_bot.bat                          # single pass, paper mode
run_bot.bat --run-loop               # 15-sec repricing / 30-min weather refresh, paper mode (recommended)
run_bot.bat --run-loop --dry-run=false   # live trading
```

What it does each pass:
1. Refreshes the cached NWS forecast for NYC when the weather interval expires
2. Looks up calibration (`beta_f`, `sigma_f`) for the station and lead time
3. Fetches CLOB orderbook and fee rate for all 11 bins
4. Scores each bin with `EV_NO = (1−P_model) − ask × (1 + fee_rate × (1−ask))`
5. Writes `execution_plan.json` with qualifying bins
6. Automatically calls `execute.py` to record paper positions (or submit live orders)
7. Reprices every 15 seconds and refreshes weather every 30 minutes

Stops automatically when the market closes (noon UTC). Switches to tomorrow's market the next day.

---

### `settle.py` — Daily settlement runner
**Location:** `poly-weather/settle.py`  
**When:** Next morning after 9am ET (after Wunderground publishes the finalized daily high)

```powershell
cd F:\Polymark\poly-weather

python settle.py                     # settle yesterday's market
python settle.py --date 2026-04-19   # settle a specific date
python settle.py --backfill 30       # backfill last 30 days of settlements
```

What it does:
1. Queries Polymarket Gamma API for the winning bin of yesterday's market
2. Closes all open positions for that date:
   - Bin matched the winner → NO loses → `pnl = -(shares × entry_price × (1 + fee_rate × (1 − entry_price)))`
   - Bin did not match → NO wins → `pnl = shares × (1 − entry_price × (1 + fee_rate × (1 − entry_price)))`
3. Writes PnL to the `positions` table in SQLite
4. Exports fresh `open_positions.json` (so the bot skips already-settled bins)
5. Prints a summary of open/closed counts and total PnL

---

### `status.py` — Position and PnL viewer
**Location:** `poly-weather/status.py`  
**When:** Anytime

```powershell
cd F:\Polymark\poly-weather

python status.py                     # show all positions
python status.py --open              # open positions only
python status.py --closed            # closed positions with PnL
python status.py --date 2026-04-19   # filter by target date
python status.py --closed --date 2026-04-19
```

Output columns: `Bin | Date | Ask | Shares | Stake | PnL | Status | Type (paper/live)`

Bottom row shows total staked, total PnL, and ROI%.

---

### `execute.py` — Order submission bridge
**Location:** `poly-weather/execute.py`  
**When:** Called automatically by the bot — rarely run manually

```powershell
python execute.py                          # reads execution_plan.json
python execute.py --plan path/to/plan.json
python execute.py --dry-run               # force paper mode
```

What it does:
- Reads `execution_plan.json` written by the Rust bot
- In **dry-run mode**: records paper positions directly to the DB with `is_paper=true`
- In **live mode**: authenticates via `POLYMARKET_PRIVATE_KEY` + `POLYMARKET_FUNDER`, submits GTC NO-buy orders via `py-clob-client`, records live positions
- Exports `open_positions.json` from the DB after each run

---

## Weekly Maintenance

### `calibrate.py` — Calibration re-fitter
**Location:** `poly-weather/calibrate.py`  
**When:** Weekly (Sunday morning, after new settlement data accumulates)

```powershell
cd F:\Polymark\poly-weather

python calibrate.py
python calibrate.py --db db/data.db --out calibration.json
```

What it does:
1. Queries `forecast_records` joined with `settlement_records` from SQLite
2. Fits `(beta_f, sigma_f)` per `(station_code, month, lead_bin)` using OLS
3. Applies fallback chain: cell → station default → global default (when < 10 samples)
4. Writes `calibration.json` — picked up by the bot at next startup

**Lead bins:** `lt6`, `6_12`, `12_24`, `24_36`, `36_48`, `48_72`, `72_plus`  
**Fallback defaults:** `beta=0.0`, `sigma=2.0`

---

### `backtest.py` — Historical backtest runner
**Location:** `poly-weather/backtest.py`  
**When:** On demand, after strategy or calibration changes

```powershell
cd F:\Polymark\poly-weather

python backtest.py --strategy base_ev --days 60 --city NYC
python backtest.py --strategy aggressive_tail --date-from 2026-02-01 --date-to 2026-04-18 --city NYC
```

What it does:
1. Loads the named strategy profile from `../poly-weather-bot/strategies.toml`
2. Replays `price_history` against `settlement_records`
3. Filters the run by `--city` when provided
4. Stores run metadata in the `backtests` table
5. Stores simulated trades in `positions` with a `backtest_id`
6. Prints a JSON summary that the dashboard API also consumes

This is the single supported backtest entry point in the repo.

---

### `backtest_param_finetune/param_sweep.py` — Parameter sweep runner
**Location:** `poly-weather/backtest_param_finetune/param_sweep.py`  
**When:** On demand, when comparing EV thresholds, position sizes, and stake caps

```powershell
cd F:\Polymark\poly-weather

python backtest_param_finetune\param_sweep.py `
  --nyc-all-available `
  --sweep-name nyc_all_available_full_v1 `
  --replay-mode single `
  --rank-by roi `
  --min-trades 10
```

For a custom range/grid:

```powershell
python backtest_param_finetune\param_sweep.py `
  --sweep-name nyc_april_single_v1 `
  --city NYC `
  --date-from 2026-04-01 `
  --date-to 2026-04-19 `
  --replay-mode single `
  --min-ev-thresholds 0.02,0.03,0.04,0.05 `
  --desired-shares 5,10 `
  --funding-pcts 0.5,1.0 `
  --max-stake-usd 25,50 `
  --rank-by roi `
  --min-trades 5
```

What it does:
1. Builds a grid over `min_ev_threshold`, `desired_shares`, `funding_pct`, and `max_stake_usd`
2. Calls the existing `run_backtest(...)` engine for each combination
3. Persists each combination as a normal backtest row with a generated `backtest_id`
4. With `--nyc-all-available`, discovers all currently backtest-ready NYC dates in `db/data.db`
5. Writes ranked CSV and Markdown reports to `backtest_param_finetune/results/`
6. Prints the top rows as JSON for quick terminal inspection

Use a train/validation split when selecting final parameters. Do not tune and
report performance on the same date range.

---

### `run_recorder.py` — Market snapshot recorder
**Location:** `poly-weather/run_recorder.py`  
**When:** Already running as a background process (captures 5-minute market snapshots, 30-minute NWS forecasts, hourly Open-Meteo forecasts)

Records market snapshots (YES/NO prices, orderbook depth, fee-rate context, volume) to `market_snapshots`. Historical CLOB backfills now preserve raw minute prices in `price_history_1m` and maintain hourly rollups in `price_history` for the current backtest flow. Used as training and replay data, not directly for live order placement.

---

## One-Time Setup / Backfill

### `settle.py --backfill N`
Pulls settlement data for the last N days. Run once at setup to populate `settlement_records` before fitting calibration.

```powershell
python settle.py --backfill 90
```

### `data/historical_forecast_backfill.py`
Backfills `forecast_records` from Open-Meteo's historical API. Run once at setup to have enough data for calibration fitting.

### `data/asos_daily_high_backfill.py`
Backfills observed daily highs from ASOS (airport surface observations). Used for model validation.

---

## Environment Variables (Live Trading)

| Variable | Description |
|----------|-------------|
| `POLYMARKET_PRIVATE_KEY` | Polygon wallet private key (`0x`-prefixed) |
| `POLYMARKET_FUNDER` | Wallet address (`0x`-prefixed) |
| `DRY_RUN` | Set to `false` to enable live order submission |
| `MIN_EV_THRESHOLD` | Override EV threshold (default `0.10`) |
| `DESIRED_SHARES` | Shares per qualifying bin (default `5`) |
| `ORDER_SIZE_USD` | Dollar stake per bin (default `5.0`) |
| `MAX_TOTAL_STAKE_USD` | Max stake per pass (default `5.0`) |
| `MAX_OPEN_POSITIONS` | Stop opening new trades above this count (default `8`) |
| `POLL_INTERVAL_SECS` | Seconds between market evaluation passes (default `15`) |
| `FORECAST_REFRESH_SECS` | Seconds between weather refreshes (default `1800`) |
| `STATION_CODE` | Calibration lookup key (default `KLGA`) |
| `CITY` | Polymarket city slug (default `nyc`) |

---

## Suggested Daily Routine

| Time | Action |
|------|--------|
| ~8am ET (day before) | `run_bot.bat --run-loop` starts (36h window opens) |
| Every 15 sec | Bot reevaluates live books, writes plan, and calls `execute.py` only when trades qualify |
| Every 30 min | Bot refreshes the NWS forecast cache |
| Noon UTC (8am ET) | Market closes, bot sleeps until next day |
| ~9am ET next morning | `python settle.py` |
| Anytime | `python status.py` |
| Sunday morning | `python calibrate.py` |
