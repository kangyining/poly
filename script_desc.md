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
run_bot.bat --run-loop               # 5-min loop, paper mode (recommended)
run_bot.bat --run-loop --dry-run=false   # live trading
```

What it does each pass:
1. Fetches NWS live forecast for NYC
2. Looks up calibration (`beta_f`, `sigma_f`) for the station and lead time
3. Fetches CLOB orderbook and fee rate for all 11 bins
4. Scores each bin with `EV_NO = (1−P_model) × (1−fee×P_model) − ask`
5. Writes `execution_plan.json` with qualifying bins
6. Automatically calls `execute.py` to record paper positions (or submit live orders)
7. Sleeps 5 minutes and repeats

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
   - Bin matched the winner → NO loses → `pnl = -stake_usd`
   - Bin did not match → NO wins → `pnl = shares × (1 − entry_price)`
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

### `run_recorder.py` — Market snapshot recorder
**Location:** `poly-weather/run_recorder.py`  
**When:** Already running as a background process (captures hourly price history)

Records market snapshots (YES/NO prices, orderbook depth, volume) to the `market_snapshots` and `price_history` tables. Used as training data for calibration and future model improvement. Not directly related to live trading.

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
| `MIN_EV_THRESHOLD` | Override EV threshold (default `0.02`) |
| `DESIRED_SHARES` | Shares per qualifying bin (default `10`) |
| `ORDER_SIZE_USD` | Dollar stake per bin (default `5.0`) |
| `MAX_TOTAL_STAKE_USD` | Max stake per pass (default `50.0`) |
| `MAX_OPEN_POSITIONS` | Stop opening new trades above this count (default `8`) |
| `POLL_INTERVAL_SECS` | Seconds between loop passes (default `300`) |
| `STATION_CODE` | Calibration lookup key (default `KLGA`) |
| `CITY` | Polymarket city slug (default `nyc`) |

---

## Suggested Daily Routine

| Time | Action |
|------|--------|
| ~8am ET (day before) | `run_bot.bat --run-loop` starts (36h window opens) |
| Every 5 min | Bot evaluates, writes plan, calls execute.py |
| Noon UTC (8am ET) | Market closes, bot sleeps until next day |
| ~9am ET next morning | `python settle.py` |
| Anytime | `python status.py` |
| Sunday morning | `python calibrate.py` |
