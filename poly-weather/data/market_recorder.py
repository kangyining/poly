"""
Market snapshot recorder — fetches Polymarket price data every 5 minutes.

For each validated weather contract:
  1. Discover active Polymarket weather events
  2. For each bin, record YES/NO prices + order book depth on the NO token
"""

import json
import logging
from datetime import date, datetime, timedelta, timezone

from data.db import Database
from data.gamma_client import (
    parse_markets,
    fetch_order_book, fetch_fee_rate, effective_ask_vwap, book_depth, best_price,
)
from data.models import MarketSnapshot
from data.weather_contracts import discover_weather_contracts

log = logging.getLogger(__name__)
DEFAULT_VWAP_SHARES = 10.0


def _hours_to_close(ts: datetime, target_date: str) -> float:
    close = datetime.fromisoformat(target_date).replace(
        hour=12, minute=0, second=0, tzinfo=timezone.utc
    )
    return (close - ts).total_seconds() / 3600


def _dates_to_record() -> list[date]:
    """Today and tomorrow — both may have active markets."""
    today = datetime.now(timezone.utc).date()
    return [today, today + timedelta(days=1)]


def record_market_snapshots(db: Database):
    """Fetch and store price snapshots for all active markets."""
    now = datetime.now(timezone.utc)
    total_saved = 0

    try:
        discoveries = discover_weather_contracts(target_dates=_dates_to_record(), include_skipped=True)
    except Exception as e:
        log.error("Weather contract discovery failed: %s", e)
        return

    for discovery in discoveries:
        contract = discovery.contract
        db.upsert_weather_contract(contract)

        if not discovery.is_valid:
            log.info(
                "Skipping weather contract: slug=%s reason=%s",
                contract.event_slug, contract.skip_reason,
            )
            continue

        event = discovery.event
        if not event:
            continue

        # Skip already-closed events (settled) — no live prices
        if event.get("closed") and not _all_markets_open(event):
            continue

        event_id   = str(event.get("id", ""))
        event_slug = event.get("_slug") or contract.event_slug
        markets    = parse_markets(event)

        for mkt in markets:
            # Fetch order book on NO token for depth data
            no_book = {}
            fee_rate = None
            no_vwap_10 = None
            if mkt["token_id_no"]:
                no_book = fetch_order_book(mkt["token_id_no"])
                fee_rate = fetch_fee_rate(mkt["token_id_no"])
                no_vwap_10 = effective_ask_vwap(no_book, DEFAULT_VWAP_SHARES)

            # Fetch order book on YES token for yes-side prices
            yes_book = {}
            if mkt["token_id_yes"]:
                yes_book = fetch_order_book(mkt["token_id_yes"])

            snap = MarketSnapshot(
                timestamp=now,
                city=contract.display_city,
                target_date=contract.target_date,
                hours_to_close=_hours_to_close(now, contract.target_date),
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
                no_asks_json=json.dumps(no_book.get("asks", [])) if no_book else None,
                fee_rate=fee_rate,
                no_vwap_10=no_vwap_10,
                volume_24h=mkt["volume_24h"],
                liquidity=mkt["liquidity"],
                market_type=contract.market_type,
                measure=contract.measure,
                unit=contract.unit,
                city_slug=contract.city_slug,
                station_id=contract.station_id,
                source_url=contract.source_url,
                validation_status=contract.validation_status,
                skip_reason=contract.skip_reason,
            )
            db.insert_snapshot(snap)
            total_saved += 1

        log.info("Snapshots: %s %s — %d bins", contract.display_city, contract.target_date, len(markets))

    log.info("Market snapshots total saved: %d", total_saved)


def _all_markets_open(event: dict) -> bool:
    """True if any market in the event is still accepting orders."""
    return any(
        not m.get("closed", True)
        for m in event.get("markets", [])
    )
