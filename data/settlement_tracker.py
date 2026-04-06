"""
Settlement tracker — two jobs:

1. daily_check(): scan recent closed markets and record winning bins
2. backfill(start, end): pull historical settled markets for Phase 0
   gate requirement (≥50 settled markets across cities)
"""

import json
import logging
from datetime import date, timedelta, datetime, timezone
from typing import Optional

from config.cities import CITIES
from data.db import Database
from data.gamma_client import fetch_event, parse_markets, winning_bin
from data.models import SettlementRecord

log = logging.getLogger(__name__)


def _extract_settlement(event: dict, city: str, target_date: str,
                        cfg: dict) -> Optional[SettlementRecord]:
    """Build a SettlementRecord from a closed event. Returns None if not resolved."""
    markets = event.get("markets", [])
    if not markets:
        return None

    win_bin = winning_bin(event)
    if not win_bin:
        # Market closed but resolution not yet visible in prices
        log.debug("No winner yet for %s %s", city, target_date)
        return None

    # Snapshot all outcome prices for audit
    outcome_snapshot = {
        m.get("groupItemTitle", ""): m.get("outcomePrices", [])
        for m in markets
    }

    # Try to infer settlement temp from winning bin label
    settlement_temp = _parse_bin_midpoint(win_bin)

    # Resolution time from event
    closed_time = event.get("closedTime") or event.get("updatedAt")
    resolved_at = None
    if closed_time:
        try:
            resolved_at = datetime.fromisoformat(
                closed_time.replace(" ", "T").rstrip("+00").rstrip("Z")
            ).replace(tzinfo=timezone.utc)
        except ValueError:
            pass

    return SettlementRecord(
        city=city,
        target_date=target_date,
        event_id=str(event.get("id", "")),
        event_slug=event.get("_slug", ""),
        winning_bin=win_bin,
        settlement_temp_f=settlement_temp,
        wunderground_station=cfg.get("wunderground_station", cfg.get("icao", "")),
        resolved_at=resolved_at,
        raw_outcome_prices=json.dumps(outcome_snapshot),
    )


def _parse_bin_midpoint(bin_label: str) -> Optional[float]:
    """
    Extract midpoint temperature from a bin label for reference.
    "38-39°F" → 38.5,  "≤27°F" → 27.0,  "≥46°F" → 46.0
    """
    label = bin_label.replace("°F", "").strip()
    if label.startswith("≤") or label.startswith("<="):
        try:
            return float(label.lstrip("≤<=").strip())
        except ValueError:
            return None
    if label.startswith("≥") or label.startswith(">="):
        try:
            return float(label.lstrip("≥>=").strip())
        except ValueError:
            return None
    if "-" in label:
        try:
            lo, hi = label.split("-")
            return (float(lo) + float(hi)) / 2
        except ValueError:
            return None
    return None


def daily_check(db: Database):
    """
    Check yesterday's markets for each city and record settlements.
    Run once per day (e.g. at 06:00 UTC, after Wunderground data is available).
    """
    yesterday = (datetime.now(timezone.utc).date() - timedelta(days=1))
    saved = 0

    for city, cfg in CITIES.items():
        date_str = yesterday.isoformat()
        if db.settlement_exists(city, date_str):
            continue

        try:
            event = fetch_event(cfg["polymarket_slug_city"], yesterday)
        except Exception as e:
            log.error("Settlement check failed %s %s: %s", city, date_str, e)
            continue

        if not event or not event.get("closed"):
            log.debug("Market not yet closed: %s %s", city, date_str)
            continue

        rec = _extract_settlement(event, city, date_str, cfg)
        if rec:
            db.insert_settlement(rec)
            log.info("Settlement saved: %s %s -> %s", city, date_str, rec.winning_bin)
            saved += 1

    log.info("Daily settlement check: saved %d new records", saved)


def backfill(db: Database, days_back: int = 60):
    """
    Pull historical settled markets going back `days_back` days.
    Used to reach the Phase 0 gate of ≥50 settled markets.

    Iterates all cities × all dates in range, skips already stored.
    """
    today = datetime.now(timezone.utc).date()
    saved = 0
    skipped = 0

    for city, cfg in CITIES.items():
        for offset in range(1, days_back + 1):
            target = today - timedelta(days=offset)
            date_str = target.isoformat()

            if db.settlement_exists(city, date_str):
                skipped += 1
                continue

            try:
                event = fetch_event(cfg["polymarket_slug_city"], target)
            except Exception as e:
                log.warning("Backfill fetch failed %s %s: %s", city, date_str, e)
                continue

            if not event:
                log.debug("No market for %s %s", city, date_str)
                continue

            if not event.get("closed"):
                log.debug("Market not yet closed: %s %s", city, date_str)
                continue

            rec = _extract_settlement(event, city, date_str, cfg)
            if rec:
                db.insert_settlement(rec)
                log.info("Backfill: %s %s -> %s", city, date_str, rec.winning_bin)
                saved += 1

    count = db.settlement_count()
    log.info(
        "Backfill complete: +%d saved, %d skipped, %d total in DB",
        saved, skipped, count,
    )
    return count
