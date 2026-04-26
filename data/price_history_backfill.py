"""
Price history backfill — fetches CLOB historical prices for all settled markets.

Closes the biggest Phase 1 gap: we have 178 (forecast, outcome) pairs but
zero historical market prices. This script retrieves them retroactively.

Flow:
  1. Load all settlement records from DB
  2. For each city+date, fetch the Gamma event to get token IDs per bin
  3. For each bin's YES token, call CLOB /prices-history (1-min resolution)
  4. Store raw 1-minute points in price_history_1m
  5. Downsample to hourly buckets (last price per hour) for price_history

Result:
  - raw minute history for future interval-replay backtests
  - hourly rollups for the current backtest flow
Runtime: ~12 minutes (rate-limited to be polite)

Idempotent: skips city+date pairs already present in both price_history_1m and
price_history.
"""

import logging
import time
from datetime import datetime, timezone, timedelta

import requests

from config.cities import CITIES
from data.db import Database
from data.gamma_client import fetch_event, parse_markets

log = logging.getLogger(__name__)

CLOB_BASE = "https://clob.polymarket.com"


def _hours_to_close(ts: datetime, target_date: str) -> float:
    """Hours from ts until noon UTC on target_date (negative = after close)."""
    close = datetime.fromisoformat(target_date).replace(
        hour=12, minute=0, second=0, tzinfo=timezone.utc
    )
    return (close - ts).total_seconds() / 3600


def _fetch_price_history(token_id: str, start_ts: int, end_ts: int,
                          retries: int = 3) -> list[dict]:
    """Fetch raw 1-min price history from CLOB for a token."""
    for attempt in range(retries):
        try:
            r = requests.get(
                f"{CLOB_BASE}/prices-history",
                params={"market": token_id, "startTs": start_ts, "endTs": end_ts},
                timeout=20,
            )
            r.raise_for_status()
            return r.json().get("history", [])
        except Exception as e:
            if attempt == retries - 1:
                log.warning("price-history failed for %s: %s", token_id[:16], e)
                return []
            time.sleep(2 ** attempt)
    return []


def _downsample_hourly(history: list[dict]) -> list[dict]:
    """
    Collapse 1-min points into hourly buckets.
    Takes the last observation within each UTC hour.
    Returns list of {ts: datetime, price: float}.
    """
    buckets = {}   # hour_bucket (datetime truncated to hour) -> last point
    for point in history:
        ts = datetime.fromtimestamp(point["t"], tz=timezone.utc)
        bucket = ts.replace(minute=0, second=0, microsecond=0)
        buckets[bucket] = point["p"]   # overwrite keeps last in bucket

    return [{"ts": bucket, "price": price}
            for bucket, price in sorted(buckets.items())]


def _build_minute_rows(city: str, target_date: str, mkt: dict,
                       history: list[dict]) -> list[tuple]:
    rows = []
    for point in history:
        ts = datetime.fromtimestamp(point["t"], tz=timezone.utc)
        rows.append((
            city,
            target_date,
            mkt["bin_label"],
            mkt["bin_index"],
            mkt["token_id_yes"],
            ts.isoformat(),
            point["p"],
            _hours_to_close(ts, target_date),
            "clob-backfill",
        ))
    return rows


def _build_hourly_rows(city: str, target_date: str, mkt: dict,
                       history: list[dict]) -> list[tuple]:
    rows = []
    for point in _downsample_hourly(history):
        rows.append((
            city,
            target_date,
            mkt["bin_label"],
            mkt["bin_index"],
            mkt["token_id_yes"],
            point["ts"].isoformat(),
            point["price"],
            _hours_to_close(point["ts"], target_date),
            "clob-backfill",
        ))
    return rows


def backfill_price_history(db: Database):
    """
    Backfill raw minute and hourly price history for all settled markets not yet in DB.
    """
    # Load all settlements
    rows = db.conn.execute(
        "SELECT city, target_date, event_slug FROM settlement_records "
        "ORDER BY city, target_date"
    ).fetchall()

    city_cfg = {name: cfg for name, cfg in CITIES.items()}
    total_saved = 0
    skipped = 0

    for city, target_date, event_slug in rows:
        has_hourly = db.price_history_exists(city, target_date)
        has_minute = db.price_history_minute_exists(city, target_date)
        if has_hourly and has_minute:
            skipped += 1
            continue

        cfg = city_cfg.get(city)
        if not cfg:
            log.warning("No config for city %s, skipping", city)
            continue

        # Fetch event from Gamma to get token IDs
        try:
            from datetime import date
            target = date.fromisoformat(target_date)
            event = fetch_event(cfg["polymarket_slug_city"], target)
        except Exception as e:
            log.warning("Gamma fetch failed %s %s: %s", city, target_date, e)
            continue

        if not event:
            log.debug("No event found: %s %s", city, target_date)
            continue

        markets = parse_markets(event)
        if not markets:
            log.debug("No markets parsed: %s %s", city, target_date)
            continue

        # Time window: market opens ~48h before close, ends at close
        close_dt = datetime.fromisoformat(target_date).replace(
            hour=12, minute=0, second=0, tzinfo=timezone.utc
        )
        start_ts = int((close_dt - timedelta(hours=48)).timestamp())
        end_ts   = int(close_dt.timestamp())

        saved_hourly = 0
        saved_minute = 0
        for mkt in markets:
            token_yes = mkt["token_id_yes"]
            if not token_yes:
                continue

            history = _fetch_price_history(token_yes, start_ts, end_ts)
            if not history:
                log.debug("No history: %s %s bin=%s", city, target_date, mkt["bin_label"])
                time.sleep(0.2)
                continue

            if not has_minute:
                minute_rows = _build_minute_rows(city, target_date, mkt, history)
                db.insert_price_history_minute_rows(minute_rows)
                saved_minute += len(minute_rows)

            if not has_hourly:
                hourly_rows = _build_hourly_rows(city, target_date, mkt, history)
                db.insert_price_history_rows(hourly_rows)
                saved_hourly += len(hourly_rows)

            time.sleep(0.15)   # gentle rate limiting per token

        total_saved += saved_hourly + saved_minute
        log.info(
            "Price history: %s %s — %d bins, %d raw minute / %d hourly points saved",
            city, target_date, len(markets), saved_minute, saved_hourly
        )
        time.sleep(0.3)   # extra pause between markets

    log.info("Price history backfill complete: +%d rows saved, %d markets skipped",
             total_saved, skipped)
    return total_saved
