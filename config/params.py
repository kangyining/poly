# config/params.py
# Strategy parameters — all values to be tuned via Phase 1 backtest.
# Defaults below are informed by Phase 0 findings (2026-04-06).
# DO NOT hand-tune these — run param_optimizer.py and fill from results.

# ── Forecast MAE (Phase 0 verified, 178 settlements, Feb–Apr 2026) ────────────
# Source: Open-Meteo historical forecast API vs settlement outcomes
# Used to set a data-driven lower bound on exclusion_range per city.
#
#   Chicago: GFS is better  (MAE 2.4°F vs ECMWF 3.9°F)
#   Dallas:  ECMWF is better (MAE 2.4°F vs GFS 4.6°F)
#   NYC:     GFS is better  (MAE 1.6°F vs ECMWF 3.2°F)
#
# Rule of thumb: exclusion_range should be ≥ 2× best_model_MAE
# to cover ~95% of forecast errors (assuming roughly normal distribution).
FORECAST_MAE_F = {
    "Chicago": {"ecmwf": 3.9, "gfs": 2.4, "best": 2.4},
    "Dallas":  {"ecmwf": 2.4, "gfs": 4.6, "best": 2.4},
    "NYC":     {"ecmwf": 3.2, "gfs": 1.6, "best": 1.6},
}

# Best model per city (use this model's forecast as primary signal)
BEST_MODEL = {
    "Chicago": "gfs",
    "Dallas":  "ecmwf",
    "NYC":     "gfs",
}

# ── Exclusion range ───────────────────────────────────────────────────────────
# Bins within ±EXCLUSION_RANGE_F of the forecast mean are excluded from trading.
# Placeholder: 2× best MAE ≈ 3–5°F. Tune via backtest sweep [3, 4, 5, 6, 7, 8].
# Chicago/Dallas: 2 × 2.4 ≈ 4.8 → start at 5.0°F
# NYC:            2 × 1.6 ≈ 3.2 → start at 4.0°F (conservative)
EXCLUSION_RANGE_F = {
    "Chicago": 5.0,
    "Dallas":  5.0,
    "NYC":     4.0,
}
EXCLUSION_RANGE_F_DEFAULT = 5.0   # fallback for cities not listed above

# ── NO token price bounds ─────────────────────────────────────────────────────
# Only buy NO tokens priced within [MIN_NO_PRICE, MAX_NO_PRICE].
# Lower bound: below 0.90 means YES is pricing at >10% — forecast may disagree.
# Upper bound: above 0.99 leaves <1¢ profit per share — not worth the capital lock.
# Tune via backtest sweep.
MIN_NO_PRICE = 0.90
MAX_NO_PRICE = 0.99

# ── Position sizing ───────────────────────────────────────────────────────────
# Per-bin position size in USDC. Small during Phase 1 validation.
# Tune based on liquidity observed: book_depth_bid must be ≥ 5× POSITION_SIZE.
POSITION_SIZE_USDC = 5.0

# ── Risk limits ───────────────────────────────────────────────────────────────
MAX_CITY_EXPOSURE   = 0.15    # max fraction of bankroll in one city
MAX_DAILY_LOSS      = 0.20    # daily loss circuit breaker (fraction of bankroll)
MAX_SINGLE_POSITION = 0.05    # max fraction of bankroll in one bin

# ── Fee parameters (Phase 0 verified) ────────────────────────────────────────
# Source: Gamma API feeSchedule.rate + Fees.sol contract
# Formula: fee = shares × TAKER_FEE_RATE × p × (1 − p)
# At NO=0.97: fee ≈ 0.15% — negligible for distant bins.
# At NO=0.70: fee ≈ 1.05% — meaningful, include in backtest P&L.
TAKER_FEE_RATE  = 0.05    # confirmed: feeSchedule.rate = 0.05
MAKER_FEE_RATE  = 0.00
MAKER_REBATE    = 0.25    # maker receives 25% of counterparty's taker fee

# ── Market structure (Phase 0 verified) ──────────────────────────────────────
BINS_PER_CITY       = 11      # confirmed for all 3 cities
BIN_WIDTH_F         = 2       # bin width in °F (e.g. "38-39°F" covers 2 integers)
TRADING_CLOSE_UTC   = 12      # hour: market stops accepting orders at 12:00 UTC
MARKET_OPEN_DAYS    = 2       # markets open ~24-36h before target date

# ── Scheduler intervals ───────────────────────────────────────────────────────
MARKET_SNAPSHOT_INTERVAL_H  = 1
METAR_INTERVAL_H             = 1
TAF_INTERVAL_H               = 1
FORECAST_INTERVAL_H          = 6
NWS_INTERVAL_H               = 6
SETTLEMENT_CHECK_UTC_HOUR    = 6   # run daily at 06:00 UTC
