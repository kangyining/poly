"""
Settlement tracker — two jobs:

1. daily_check(): scan recent closed markets and record winning bins,
   close any matching open positions with PnL, and export open_positions.json
2. backfill(start, end): pull historical settled markets for Phase 0
   gate requirement (≥50 settled markets across cities)
"""

import json
import logging
from datetime import date, timedelta, datetime, timezone
from pathlib import Path
from typing import Optional

from config.cities import CITIES
from data.db import Database
from data.gamma_client import fetch_event, parse_markets, winning_bin
from data.models import SettlementRecord

# Path written for the Rust bot to read on its next run
OPEN_POSITIONS_JSON = Path(__file__).parent.parent.parent / "poly-weather" / "open_positions.json"

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


def _settle_positions(db: Database, target_date: str, winning_bin_label: str,
                      resolved_at: Optional[datetime]) -> int:
    """
    Close any open positions for target_date whose bin matches the winning bin.
    NO tokens on the winning bin pay 0 (NO loses); all other bins pay 1/share.
    Returns number of positions closed.
    """
    settled_ts = resolved_at.isoformat() if resolved_at else datetime.now(timezone.utc).isoformat()
    open_positions = db.get_open_positions()
    closed = 0

    for pos in open_positions:
        if pos["target_date"] != target_date:
            continue

        # NO token wins if the actual outcome is a DIFFERENT bin
        if pos["bin_label"] == winning_bin_label:
            # This bin won (YES), so NO loses — full cost basis including USDC fee
            fee_rate = pos.get("fee_rate") or 0.0
            pnl = -(pos["shares"] * pos["entry_price"] * (1.0 + fee_rate * (1.0 - pos["entry_price"])))
        else:
            # This bin lost (YES), so NO wins — each share pays $1.
            # Fee was paid in USDC at trade time: fee/share = fee_rate × entry_price × (1 - entry_price)
            # Total cost/share = entry_price × (1 + fee_rate × (1 - entry_price))
            fee_rate = pos.get("fee_rate") or 0.0
            pnl = pos["shares"] * (1.0 - pos["entry_price"] * (1.0 + fee_rate * (1.0 - pos["entry_price"])))

        db.close_position(pos["id"], round(pnl, 4), settled_ts)
        log.info(
            "Position closed: bin=%s target=%s pnl=%.4f is_paper=%s",
            pos["bin_label"], target_date, pnl, bool(pos["is_paper"]),
        )
        closed += 1

    return closed


def daily_check(db: Database,
                open_positions_json: Path = OPEN_POSITIONS_JSON,
                target_date=None):
    """
    Check markets for each city and record settlements.
    Closes matching open positions with PnL and exports open_positions.json.

    target_date: date object to check. Defaults to yesterday.
    Detects resolution via winning_bin() (outcome prices reaching 1.0),
    not the `closed` flag — Polymarket sometimes resolves without setting closed=True.
    """
    check_date = target_date or (datetime.now(timezone.utc).date() - timedelta(days=1))
    saved = 0
    positions_closed = 0

    for city, cfg in CITIES.items():
        date_str = check_date.isoformat()
        if db.settlement_exists(city, date_str):
            log.debug("Settlement already recorded: %s %s", city, date_str)
            continue

        try:
            event = fetch_event(cfg["polymarket_slug_city"], check_date)
        except Exception as e:
            log.error("Settlement check failed %s %s: %s", city, date_str, e)
            continue

        if not event:
            log.debug("No event found: %s %s", city, date_str)
            continue

        # Use winning_bin as the resolution signal — more reliable than closed flag
        if not winning_bin(event):
            log.info("Market not yet resolved (no winner): %s %s", city, date_str)
            continue

        rec = _extract_settlement(event, city, date_str, cfg)
        if rec:
            db.insert_settlement(rec)
            log.info("Settlement saved: %s %s -> %s", city, date_str, rec.winning_bin)
            saved += 1

            if rec.winning_bin:
                n = _settle_positions(db, date_str, rec.winning_bin, rec.resolved_at)
                positions_closed += n

    # Always export current open positions so Rust bot has a fresh view
    db.export_open_positions_json(open_positions_json)

    log.info(
        "Daily settlement check: saved=%d settlements, closed=%d positions",
        saved, positions_closed,
    )


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
