-- Polymarket Weather Bot — SQLite Schema
-- Phase 0 verified conventions:
--   temps in Fahrenheit (Polymarket settles in whole °F via Wunderground)
--   settlement stations: Chicago→KORD, Dallas→KDAL, NYC→KLGA

CREATE TABLE IF NOT EXISTS market_snapshots (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp       TEXT NOT NULL,        -- ISO 8601 UTC
    city            TEXT NOT NULL,
    target_date     TEXT NOT NULL,        -- YYYY-MM-DD
    hours_to_close  REAL,                 -- hours until noon UTC on target_date
    event_id        TEXT,
    event_slug      TEXT,
    bin_label       TEXT NOT NULL,        -- "38-39°F"
    bin_index       INTEGER,              -- 0 = lowest bin
    token_id_yes    TEXT,
    token_id_no     TEXT,
    yes_price       REAL,
    no_price        REAL,
    yes_best_bid    REAL,
    yes_best_ask    REAL,
    no_best_bid     REAL,
    no_best_ask     REAL,
    book_depth_bid  REAL,                 -- USDC on NO bid side
    book_depth_ask  REAL,                 -- USDC on NO ask side
    no_asks_json    TEXT,                 -- raw NO ask ladder snapshot [{price,size}, ...]
    fee_rate        REAL,                 -- token fee rate at snapshot time (decimal, e.g. 0.10)
    no_vwap_10      REAL,                 -- convenience VWAP ask for 10 shares on NO side
    volume_24h      REAL,
    liquidity       REAL
);

CREATE TABLE IF NOT EXISTS metar_records (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp       TEXT NOT NULL,        -- ISO 8601 UTC (when fetched)
    icao            TEXT NOT NULL,        -- KORD / KDAL / KLGA
    city            TEXT NOT NULL,
    temp_c          REAL,
    temp_f          REAL,
    dewpoint_c      REAL,
    obs_time        TEXT,                 -- METAR observation time
    raw_metar       TEXT
);

CREATE TABLE IF NOT EXISTS forecast_records (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp           TEXT NOT NULL,
    city                TEXT NOT NULL,
    target_date         TEXT NOT NULL,
    source              TEXT NOT NULL,    -- "open-meteo"
    temp_max_ecmwf_c    REAL,
    temp_max_gfs_c      REAL,
    temp_max_ecmwf_f    REAL,
    temp_max_gfs_f      REAL,
    model_spread_c      REAL,             -- |ecmwf - gfs|
    forecast_lead_hours REAL,
    raw_json            TEXT
);

CREATE TABLE IF NOT EXISTS settlement_records (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    city                 TEXT NOT NULL,
    target_date          TEXT NOT NULL,
    event_id             TEXT,
    event_slug           TEXT,
    winning_bin          TEXT,            -- "38-39°F"
    settlement_temp_f    REAL,
    wunderground_station TEXT,
    resolved_at          TEXT,
    raw_outcome_prices   TEXT             -- JSON
);

CREATE TABLE IF NOT EXISTS taf_records (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp       TEXT NOT NULL,        -- when fetched (ISO 8601 UTC)
    icao            TEXT NOT NULL,        -- KORD / KDAL / KLGA
    city            TEXT NOT NULL,
    issue_time      TEXT,                 -- TAF issue time (ISO 8601 UTC)
    valid_from      TEXT,                 -- valid period start (ISO 8601 UTC)
    valid_to        TEXT,                 -- valid period end (ISO 8601 UTC)
    raw_taf         TEXT                  -- full raw TAF string
);

CREATE TABLE IF NOT EXISTS price_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    city            TEXT NOT NULL,
    target_date     TEXT NOT NULL,        -- YYYY-MM-DD
    bin_label       TEXT NOT NULL,        -- "38-39°F"
    bin_index       INTEGER,
    token_id_yes    TEXT,
    timestamp       TEXT NOT NULL,        -- ISO 8601 UTC (hourly bucket)
    yes_price       REAL,                 -- NO price = 1 - yes_price (neg-risk)
    hours_to_close  REAL,                 -- hours until noon UTC on target_date
    source          TEXT DEFAULT 'clob-backfill'
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_price_history_unique
    ON price_history(city, target_date, bin_label, timestamp);

CREATE INDEX IF NOT EXISTS idx_price_history_city_date
    ON price_history(city, target_date);

CREATE TABLE IF NOT EXISTS nws_forecasts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp       TEXT NOT NULL,        -- when fetched (ISO 8601 UTC)
    city            TEXT NOT NULL,
    target_date     TEXT NOT NULL,        -- YYYY-MM-DD
    period_name     TEXT,                 -- "Monday", "Monday Night", etc.
    is_daytime      INTEGER,              -- 1 = daytime period
    temp_f          REAL,                 -- forecast temperature °F
    short_forecast  TEXT,                 -- "Partly Cloudy", etc.
    forecast_lead_hours REAL
);

CREATE TABLE IF NOT EXISTS positions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    token_id        TEXT NOT NULL,
    market_id       TEXT NOT NULL,
    bin_label       TEXT NOT NULL,
    event_slug      TEXT NOT NULL,
    target_date     TEXT NOT NULL,        -- YYYY-MM-DD
    entry_price     REAL NOT NULL,        -- effective ask paid (NO side)
    shares          REAL NOT NULL,
    stake_usd       REAL NOT NULL,
    order_id        TEXT,                 -- CLOB order ID (null for paper)
    status          TEXT NOT NULL DEFAULT 'open',  -- open | closed | cancelled
    pnl             REAL,                 -- USDC PnL (null until settled)
    opened_at       TEXT NOT NULL,        -- ISO 8601 UTC
    settled_at      TEXT,                 -- ISO 8601 UTC (null until closed)
    is_paper        INTEGER NOT NULL DEFAULT 1,  -- 1=paper, 0=live
    strategy_name   TEXT NOT NULL DEFAULT 'default',
    backtest_id     TEXT,                 -- NULL for live/paper trades
    trade_rationale TEXT                  -- JSON: ev, p_model, fee_rate, etc.
);

CREATE TABLE IF NOT EXISTS backtests (
    id            TEXT PRIMARY KEY,       -- UUID
    strategy      TEXT NOT NULL,
    params        TEXT NOT NULL,          -- JSON: all tunable params
    date_from     TEXT NOT NULL,          -- YYYY-MM-DD
    date_to       TEXT NOT NULL,          -- YYYY-MM-DD
    created_at    TEXT NOT NULL,          -- ISO 8601 UTC
    total_trades  INTEGER,
    win_rate      REAL,
    total_pnl     REAL,
    sharpe        REAL,
    status        TEXT NOT NULL DEFAULT 'running'  -- running | done | failed
);

CREATE INDEX IF NOT EXISTS idx_positions_status
    ON positions(status);
CREATE INDEX IF NOT EXISTS idx_positions_target_date
    ON positions(target_date);
CREATE UNIQUE INDEX IF NOT EXISTS idx_positions_token_order
    ON positions(token_id, order_id);

-- Indices
CREATE INDEX IF NOT EXISTS idx_snapshots_city_date
    ON market_snapshots(city, target_date);
CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp
    ON market_snapshots(timestamp);
CREATE INDEX IF NOT EXISTS idx_metar_city_time
    ON metar_records(icao, timestamp);
CREATE INDEX IF NOT EXISTS idx_forecast_city_date
    ON forecast_records(city, target_date);
CREATE INDEX IF NOT EXISTS idx_settlement_city_date
    ON settlement_records(city, target_date);
