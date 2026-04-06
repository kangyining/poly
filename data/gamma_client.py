"""
Gamma API client — read-only market data.

Phase 0 findings:
  - ?tag=weather filter is BROKEN (silently ignored), do NOT use it
  - Access weather markets via slug:
      highest-temperature-in-{city}-on-april-{dd}-2026
  - clobTokenIds is ALSO a JSON-encoded string — must json.loads() it too
  - clobTokenIds[0] = YES token, clobTokenIds[1] = NO token
  - negRisk: true on all temperature markets
  - endDate = target_day 12:00 UTC (trading close, before daily high)
"""

import time
import logging
from datetime import date, datetime, timedelta, timezone
from typing import Optional

import requests

GAMMA_BASE = "https://gamma-api.polymarket.com"
CLOB_BASE  = "https://clob.polymarket.com"

log = logging.getLogger(__name__)


def _get(url: str, params: dict = None, retries: int = 3) -> dict | list:
    for attempt in range(retries):
        try:
            r = requests.get(url, params=params, timeout=15)
            r.raise_for_status()
            return r.json()
        except Exception as e:
            if attempt == retries - 1:
                raise
            log.warning("GET %s failed (attempt %d): %s", url, attempt + 1, e)
            time.sleep(2 ** attempt)


def build_slug(city_slug: str, target: date) -> str:
    """
    Build the Gamma event slug for a daily temperature market.
    city_slug: "chicago", "dallas", "nyc"
    """
    month = target.strftime("%B").lower()   # "april"
    day   = target.day                      # 7  (no leading zero)
    year  = target.year
    return f"highest-temperature-in-{city_slug}-on-{month}-{day}-{year}"


def fetch_event(city_slug: str, target: date) -> Optional[dict]:
    """
    Fetch one temperature event from Gamma API.
    Returns the first element of the response array, or None if not found.
    """
    slug = build_slug(city_slug, target)
    data = _get(f"{GAMMA_BASE}/events", params={"slug": slug})
    if not data:
        log.debug("No event found for slug: %s", slug)
        return None
    event = data[0] if isinstance(data, list) else data
    event["_slug"] = slug
    return event


def fetch_events_range(city_slug: str, start: date, end: date) -> list[dict]:
    """
    Fetch events for a range of dates (inclusive). Used for backfill.
    Skips dates where no market exists.
    """
    results = []
    d = start
    while d <= end:
        try:
            ev = fetch_event(city_slug, d)
            if ev:
                results.append(ev)
        except Exception as e:
            log.warning("Failed fetching %s %s: %s", city_slug, d, e)
        d += timedelta(days=1)
        time.sleep(0.2)   # gentle rate limiting
    return results


def parse_markets(event: dict) -> list[dict]:
    """
    Extract all bin markets from an event.
    Returns list of dicts with standardised keys.

    Note: outcomePrices is a JSON-encoded string in the API response,
    not a list — must json.loads() it.
    """
    import json as _json

    out = []
    for i, m in enumerate(event.get("markets", [])):
        # clobTokenIds is also a JSON-encoded string in the API response
        raw_token_ids = m.get("clobTokenIds", "[]")
        if isinstance(raw_token_ids, str):
            try:
                token_ids = _json.loads(raw_token_ids)
            except Exception:
                token_ids = []
        else:
            token_ids = raw_token_ids

        # outcomePrices is a JSON string e.g. '["0.0025", "0.9975"]'
        raw_prices = m.get("outcomePrices", "[]")
        if isinstance(raw_prices, str):
            try:
                prices = _json.loads(raw_prices)
            except Exception:
                prices = ["0", "0"]
        else:
            prices = raw_prices

        out.append({
            "market_id":    m.get("id"),
            "bin_label":    m.get("groupItemTitle", ""),
            "bin_index":    i,
            "token_id_yes": token_ids[0] if len(token_ids) > 0 else "",
            "token_id_no":  token_ids[1] if len(token_ids) > 1 else "",
            "yes_price":    float(prices[0]) if prices else 0.0,
            "no_price":     float(prices[1]) if len(prices) > 1 else 0.0,
            # bestBid/bestAsk are available directly on the market object
            "yes_best_bid": float(m.get("bestBid") or 0),
            "yes_best_ask": float(m.get("bestAsk") or 0),
            "last_trade":   float(m.get("lastTradePrice") or 0),
            "liquidity":    float(m.get("liquidityNum") or m.get("liquidity") or 0),
            "volume_24h":   float(m.get("volume24hr") or m.get("volume") or 0),
            "neg_risk":     bool(m.get("negRisk", True)),
            "closed":       bool(m.get("closed", False)),
        })
    return out


def fetch_order_book(token_id: str) -> dict:
    """
    Fetch CLOB order book for a single token.
    Returns {"bids": [...], "asks": [...]} or empty dict on error.
    """
    try:
        return _get(f"{CLOB_BASE}/book", params={"token_id": token_id})
    except Exception as e:
        log.warning("Order book fetch failed for %s: %s", token_id[:16], e)
        return {}


def book_depth(book: dict, side: str) -> float:
    """Sum total USDC size on bids or asks."""
    levels = book.get(side, [])
    return sum(float(lvl.get("size", 0)) for lvl in levels)


def best_price(book: dict, side: str) -> float:
    """Best bid or ask price from order book."""
    levels = book.get(side, [])
    if not levels:
        return 0.0
    prices = [float(lvl.get("price", 0)) for lvl in levels]
    return max(prices) if side == "bids" else min(prices)


def winning_bin(event: dict) -> Optional[str]:
    """
    For a closed event, return the bin label that resolved to YES.
    Identified by outcomePrices[0] == "1" (YES resolved).
    """
    import json as _json
    for m in event.get("markets", []):
        raw = m.get("outcomePrices", "[]")
        if isinstance(raw, str):
            try:
                prices = _json.loads(raw)
            except Exception:
                prices = []
        else:
            prices = raw
        if prices and float(prices[0]) >= 0.99:
            return m.get("groupItemTitle")
    return None
