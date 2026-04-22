"""
Backfill missing bins in price_history — retry CLOB for bins 7-10 that failed initially.

These are typically low-liquidity warm-side bins that may have had transient failures.
When raw minute history is enabled, this script writes both price_history_1m and
price_history for the retried bins.
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
    """Hours from ts until noon UTC on target_date."""
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
    """Collapse 1-min points into hourly buckets."""
    buckets = {}
    for point in history:
        ts = datetime.fromtimestamp(point["t"], tz=timezone.utc)
        bucket = ts.replace(minute=0, second=0, microsecond=0)
        buckets[bucket] = point["p"]
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


def backfill_missing_bins(db: Database, city: str = "NYC") -> int:
    """
    Retry CLOB price history for missing bins in price_history table.

    For each settlement, find which bins have no price data and retry those.
    """
    # Get settlements for this city
    settlements = db.conn.execute(
        "SELECT target_date, event_slug FROM settlement_records WHERE city = ? ORDER BY target_date",
        (city,)
    ).fetchall()

    if not settlements:
        log.info("No settlements found for %s", city)
        return 0

    cfg = CITIES.get(city)
    if not cfg:
        log.error("No config for city %s", city)
        return 0

    total_saved = 0

    for target_date, event_slug in settlements:
        # Get current bin coverage
        current_bins = set(
            b[0] for b in db.conn.execute(
                "SELECT DISTINCT bin_index FROM price_history WHERE city = ? AND target_date = ?",
                (city, target_date)
            ).fetchall()
        )

        # Find missing bins (should be 0-10)
        all_bins = set(range(11))
        missing_bins = all_bins - current_bins

        if not missing_bins:
            continue

        log.info("Retrying %s %s: missing bins %s", city, target_date, sorted(missing_bins))

        # Fetch event to get all markets
        try:
            from datetime import date
            target = date.fromisoformat(target_date)
            event = fetch_event(cfg["polymarket_slug_city"], target)
        except Exception as e:
            log.warning("Gamma fetch failed %s %s: %s", city, target_date, e)
            continue

        if not event:
            continue

        markets = parse_markets(event)
        if not markets:
            continue

        # Time window
        close_dt = datetime.fromisoformat(target_date).replace(
            hour=12, minute=0, second=0, tzinfo=timezone.utc
        )
        start_ts = int((close_dt - timedelta(hours=48)).timestamp())
        end_ts = int(close_dt.timestamp())

        # Retry only missing bins
        for mkt in markets:
            if mkt["bin_index"] not in missing_bins:
                continue

            token_yes = mkt["token_id_yes"]
            if not token_yes:
                continue

            history = _fetch_price_history(token_yes, start_ts, end_ts)
            if not history:
                log.debug("No history for %s %s bin=%d", city, target_date, mkt["bin_index"])
                time.sleep(0.2)
                continue

            minute_rows = _build_minute_rows(city, target_date, mkt, history)
            hourly_rows = _build_hourly_rows(city, target_date, mkt, history)
            db.insert_price_history_minute_rows(minute_rows)
            db.insert_price_history_rows(hourly_rows)

            log.info("Retry: %s %s bin %d — %d raw minute / %d hourly points saved",
                    city, target_date, mkt["bin_index"], len(minute_rows), len(hourly_rows))
            total_saved += len(minute_rows) + len(hourly_rows)
            time.sleep(0.15)

        time.sleep(0.3)

    log.info("Missing bins backfill complete: %d rows added", total_saved)
    return total_saved
