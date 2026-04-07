# Phase 0 — Missing Items & Gap Analysis

*Compiled: 2026-04-06*
*Status: Phase 0 core pipeline built. Items below must be resolved before Phase 1.*

---

## Blocking Phase 1 Entry

### 1. Recorder not proven stable for 3+ days
**Gate requirement**: scheduler loop must run continuously for 3+ days without crashing.
**Current state**: Only `--once` runs executed today. The continuous scheduler (`python run_recorder.py`) has never been started and left running.
**Fix**: Start `python run_recorder.py` and leave it running. Monitor `recorder.log` for errors.
**Status**: ❌ NOT STARTED

---

### 2. No historical market prices (60-day gap)
**Why it matters**: Phase 1 backtest needs (market_price, forecast, outcome) triples. We have outcomes (178 settlements) and forecasts (178 historical ECMWF/GFS records), but **zero** historical price data for those 60 days.
**Root cause**: Gamma API only returns current prices. There is no historical price time series endpoint.
**Consequence**: Phase 1 cannot answer "what was the NO bin priced at 2 days before resolution?" We can only approximate using current pricing behavior as a proxy, or wait 2+ more weeks of live recording to accumulate real pairs.
**Options**:
  - Accept the gap and use current observed price ranges as approximation for entry price assumptions in backtest
  - Wait 2–4 more weeks of live recording before starting Phase 1 analysis
  - Check if any third-party archives historical Polymarket CLOB data (e.g. Polymarket data dumps, Dune Analytics)
**Status**: ❌ STRUCTURAL GAP — decision needed

---

### 3. `config/params.py` not created
**Why it matters**: No single source of truth for strategy parameters. Phase 1 analysis and backtest engine need these values to be importable from one place.
**What it should contain**:
```python
# Strategy bounds (to be tuned via backtest)
EXCLUSION_RANGE_F   = 6.0     # ±°F from forecast mean to exclude
MIN_NO_PRICE        = 0.90    # minimum NO price to consider buying
MAX_NO_PRICE        = 0.99    # maximum NO price
POSITION_SIZE_USDC  = 5.0     # per-bin position size
MAX_CITY_EXPOSURE   = 0.15    # max % of bankroll in one city
MAX_DAILY_LOSS      = 0.20    # daily loss circuit breaker

# Fee parameters (verified Phase 0)
TAKER_FEE_RATE      = 0.05    # confirmed from feeSchedule.rate
MAKER_FEE_RATE      = 0.00
MAKER_REBATE_RATE   = 0.25

# Forecast MAE by city (from historical backfill, 2026-04-06)
# Used to set exclusion_range per city
FORECAST_MAE_F = {
    "Chicago": {"ecmwf": 3.9, "gfs": 2.4},
    "Dallas":  {"ecmwf": 2.4, "gfs": 4.6},
    "NYC":     {"ecmwf": 3.2, "gfs": 1.6},
}
```
**Fix**: Create `config/params.py` with placeholder values; populate from backtest results later.
**Status**: ❌ NOT CREATED

---

## Important — Not Blocking Phase 1 Start

### 4. No data quality check script
**Why it matters**: Phase 1 analysis could silently run on dirty data (missing snapshots, duplicate records, anomalous prices, cities with gaps).
**What it should check**:
  - Missing hourly snapshots (gaps >2 hours in market_snapshots)
  - Cities with fewer settlements than expected
  - Anomalous prices (yes_price + no_price far from 1.0 for non-neg-risk rows)
  - Duplicate settlement records
  - Forecast records missing for settlement dates
  - METAR gaps (stations not reporting)
**Suggested location**: `data/quality_check.py` or a notebook `notebooks/00_data_quality.ipynb`
**Status**: ❌ NOT CREATED

---

### 5. `findings/phase0_findings.md` outdated
**Why it matters**: Was written during initial API recon on 2026-04-06. Does not reflect today's subsequent work.
**Missing from current file**:
  - clobTokenIds is also a JSON-encoded string (same bug as outcomePrices, discovered in live testing)
  - ASOS = Wunderground verified (123/123 settlements, ±0.5°F is arithmetic only)
  - Forecast MAE numbers: Chicago GFS 2.4°F, Dallas ECMWF 2.4°F, NYC GFS 1.6°F
  - Open-Meteo Previous Runs API confirmed working (historical forecast backfill)
  - TAF has no temperature field (wind/cloud only)
  - NWS API works but has timeout issues (~15s, needs 30s timeout + retry)
  - CLOB order book returns 404 for closed markets (expected behavior, not a bug)
**Fix**: Update `findings/phase0_findings.md` with all post-recon discoveries.
**Status**: ❌ OUTDATED

---

### 6. Iowa Mesonet ASOS daily max backfill (P4 deferred)
**Why it matters**: We have 178 settlements verified by bin match, but not the exact daily max temperature per date (e.g., was it 82°F or 83°F within the "82-83°F" bin?). Needed for `actual_max_temp` feature in ML (Phase 1 Week 4).
**Current state**: Verified that ASOS daily max falls inside the winning bin 123/123 times. The exact number is obtainable via Iowa Mesonet ASOS historical API — same API we already use for real-time METARs, no key needed.
**Fix**: Add `data/asos_daily_max_backfill.py` and a `station_daily_highs` table. Fetch all 60 days × 3 stations.
**When needed**: Before ML feature engineering (Phase 1 Week 4), not before basic backtest (Phase 1 Week 3).
**Status**: ⏳ DEFERRED TO PHASE 1 WEEK 4

---

## Design Doc Specifies — Not Yet Built

### 7. `data/clob_client.py` missing
**Design doc**: Lists this as a separate file for CLOB API read operations.
**Current state**: CLOB order book calls are embedded in `gamma_client.py` (`fetch_order_book`, `book_depth`, `best_price`).
**Impact**: Structural debt. Not blocking Phase 0 or Phase 1, but creates a messy separation before Phase 2 execution work begins.
**Fix**: Extract CLOB functions from `gamma_client.py` into `data/clob_client.py`.
**Status**: ⏳ DEFERRED TO PHASE 2

---

### 8. Notebooks not created
**Design doc specifies**:
  - `notebooks/00_api_recon.ipynb` — API exploration (partially done in terminal, never formalized)
  - `notebooks/01_market_structure.ipynb` — answers: how many NO bins >$0.90? liquidity distribution? price change after forecast update?
**Why it matters**: `01_market_structure.ipynb` is a Phase 0b gate deliverable. Without it, we don't have quantified answers to "how many tradeable bins exist per day?" and "when is liquidity deepest?"
**Fix**: Create notebooks once we have 3+ days of live price snapshots to analyze.
**Status**: ❌ NOT CREATED — depends on live data accumulation

---

### 9. `config/.env.example` not created
**Design doc**: Lists this as a template for environment variables.
**Needed for**: Wunderground API key (future), Telegram bot token (Phase 2), Polygon wallet private key (Phase 3).
**Fix**: Create minimal template file.
**Status**: ❌ NOT CREATED (low priority)

---

## Phase 0 Gate Items from Weekly Plan

### 10. Polygon wallet + USDC deposit
**Weekly plan**: Day 1-2 item. Required before any execution work including testnet.
**Current state**: Not started.
**Fix**: Create Polygon wallet, deposit small USDC ($20-50 sufficient for testnet verification).
**Status**: ❌ NOT STARTED — manual user action required

---

### 11. negRisk=True testnet verification
**Weekly plan**: Verify the order parameter is correctly handled before real money.
**Current state**: negRisk confirmed in market data (`"negRisk": true` on all temperature markets) but never tested in an actual order submission.
**Depends on**: Item 10 (wallet setup).
**Fix**: Submit a $1 test order on testnet with `negRisk=True` and verify the position appears correctly.
**Status**: ❌ NOT STARTED — blocked by item 10

---

## Summary Table

| # | Item | Blocks Phase 1? | Effort | Priority |
|---|---|---|---|---|
| 1 | Recorder 3-day stability | ✅ YES | Start process, wait | 🔴 Critical |
| 2 | Historical market prices | ✅ YES (decision) | Strategic decision | 🔴 Critical |
| 3 | `config/params.py` | ✅ YES | 30 min | 🔴 Critical |
| 4 | Data quality script | Soft block | 2-3 hrs | 🟡 Important |
| 5 | `phase0_findings.md` update | No | 1 hr | 🟡 Important |
| 6 | ASOS daily max backfill | No (Phase 1 Wk4) | 2 hrs | 🟢 Deferred |
| 7 | `clob_client.py` split | No (Phase 2) | 1 hr | 🟢 Deferred |
| 8 | Notebooks | Soft block | 4-6 hrs | 🟡 Important |
| 9 | `.env.example` | No | 15 min | 🟢 Low |
| 10 | Polygon wallet | No (Phase 2) | Manual | 🟢 Deferred |
| 11 | negRisk testnet | No (Phase 2) | Depends on 10 | 🟢 Deferred |

---

## Recommended Sequence Before Phase 1

```
Step 1 (now):       Start python run_recorder.py — leave running
Step 2 (today):     Create config/params.py with MAE-informed defaults
Step 3 (today):     Update findings/phase0_findings.md
Step 4 (tomorrow):  Implement data quality check script
Step 5 (day 3):     Decide on historical price gap strategy
Step 6 (day 4+):    Build 01_market_structure.ipynb once 3 days of data collected
Step 7 (day 4+):    Phase 0 gate review → Go/No-Go for Phase 1
```
