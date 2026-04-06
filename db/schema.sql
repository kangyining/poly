-- Polymarket Weather Bot — SQLite Schema
-- Phase 0 verified conventions:
--   temps in Fahrenheit (Polymarket settles in whole °F via Wunderground)
--   settlement stations: Chicago→KORD, Dallas→KDAL, NYC→KLGA

CREATE TABLE IF NOT EXISTS market_snapshots (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp       TEXT NOT NULL,        -- ISO 8601 UTC
    city            TEXT NOT NULL,
    target_date     TEXT NOT NULL,        -- YYYY-MM-DD
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
