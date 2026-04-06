"""
Market snapshot recorder — fetches Polymarket price data every hour.

For each city:
  1. Fetch today's and tomorrow's temperature event (slug-based)
  2. For each bin, record YES/NO prices + order book depth on the NO token
"""

import json
import logging
from datetime import date, datetime, timedelta, timezone

from config.cities import CITIES
from data.db import Database
from data.gamma_client import (
    fetch_event, parse_markets,
    fetch_order_book, book_depth, best_price,
)
from data.models import MarketSnapshot

log = logging.getLogger(__name__)


def _dates_to_record() -> list[date]:
    """Today and tomorrow — both may have active markets."""
    today = datetime.now(timezone.utc).date()
    return [today, today + timedelta(days=1)]


def record_market_snapshots(db: Database):
    """Fetch and store price snapshots for all active markets."""
    now = datetime.now(timezone.utc)
    total_saved = 0

    for city, cfg in CITIES.items():
        slug_city = cfg["polymarket_slug_city"]

        for target in _dates_to_record():
            try:
                event = fetch_event(slug_city, target)
            except Exception as e:
                log.error("Event fetch failed %s %s: %s", city, target, e)
                continue

            if not event:
                continue

            # Skip already-closed events (settled) — no live prices
            if event.get("closed") and not _all_markets_open(event):
                continue

            event_id   = str(event.get("id", ""))
            event_slug = event.get("_slug", "")
            markets    = parse_markets(event)

            for mkt in markets:
                # Fetch order book on NO token for depth data
                no_book = {}
                if mkt["token_id_no"]:
                    no_book = fetch_order_book(mkt["token_id_no"])

                # Fetch order book on YES token for yes-side prices
                yes_book = {}
                if mkt["token_id_yes"]:
                    yes_book = fetch_order_book(mkt["token_id_yes"])

                snap = MarketSnapshot(
                    timestamp=now,
                    city=city,
                    target_date=target.isoformat(),
                    event_id=event_id,
                    event_slug=event_slug,
                    bin_label=mkt["bin_label"],
                    bin_index=mkt["bin_index"],
                    token_id_yes=mkt["token_id_yes"],
                    token_id_no=mkt["token_id_no"],
                    yes_price=mkt["yes_price"],
                    no_price=mkt["no_price"],
                    yes_best_bid=best_price(yes_book, "bids"),
                    yes_best_ask=best_price(yes_book, "asks"),
                    no_best_bid=best_price(no_book, "bids"),
                    no_best_ask=best_price(no_book, "asks"),
                    book_depth_bid=book_depth(no_book, "bids"),
                    book_depth_ask=book_depth(no_book, "asks"),
                    volume_24h=mkt["volume_24h"],
                    liquidity=mkt["liquidity"],
                )
                db.insert_snapshot(snap)
                total_saved += 1

            log.info("Snapshots: %s %s — %d bins", city, target, len(markets))

    log.info("Market snapshots total saved: %d", total_saved)


def _all_markets_open(event: dict) -> bool:
    """True if any market in the event is still accepting orders."""
    return any(
        not m.get("closed", True)
        for m in event.get("markets", [])
    )
