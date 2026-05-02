"""
Discovery and validation for normalized Polymarket weather contracts.

V1 accepts only allowlisted US Fahrenheit daily high-temperature events that
resolve to the configured Wunderground station.
"""

from __future__ import annotations

import json
import logging
import re
from dataclasses import dataclass
from datetime import date
from typing import Iterable, Optional

import requests

from config.weather_contracts import DAILY_HIGH_F_CONTRACTS, enabled_daily_high_contracts
from data.models import WeatherContract

log = logging.getLogger(__name__)

GAMMA_BASE = "https://gamma-api.polymarket.com"

DAILY_HIGH_SLUG_RE = re.compile(
    r"^highest-temperature-in-(?P<city>[a-z0-9-]+)-on-"
    r"(?P<month>[a-z]+)-(?P<day>\d{1,2})-(?P<year>\d{4})$",
    re.IGNORECASE,
)
MONTHS = {
    "january": 1,
    "february": 2,
    "march": 3,
    "april": 4,
    "may": 5,
    "june": 6,
    "july": 7,
    "august": 8,
    "september": 9,
    "october": 10,
    "november": 11,
    "december": 12,
}


@dataclass
class WeatherContractDiscovery:
    contract: WeatherContract
    event: Optional[dict]

    @property
    def is_valid(self) -> bool:
        return self.contract.validation_status == "valid"


def build_daily_high_slug(city_slug: str, target: date) -> str:
    return (
        f"highest-temperature-in-{city_slug}-on-"
        f"{target.strftime('%B').lower()}-{target.day}-{target.year}"
    )


def parse_daily_high_slug(slug: str) -> Optional[tuple[str, date]]:
    match = DAILY_HIGH_SLUG_RE.match(slug or "")
    if not match:
        return None
    month = MONTHS.get(match.group("month").lower())
    if month is None:
        return None
    try:
        target = date(int(match.group("year")), month, int(match.group("day")))
    except ValueError:
        return None
    return match.group("city").lower(), target


def fetch_event_by_slug(slug: str) -> Optional[dict]:
    response = requests.get(f"{GAMMA_BASE}/events", params={"slug": slug}, timeout=20)
    response.raise_for_status()
    payload = response.json()
    if not payload:
        return None
    event = payload[0] if isinstance(payload, list) else payload
    event["_slug"] = slug
    return event


def fetch_weather_events(limit: int = 100, max_pages: int = 5) -> list[dict]:
    events: list[dict] = []
    for page in range(max_pages):
        offset = page * limit
        response = requests.get(
            f"{GAMMA_BASE}/events",
            params={
                "limit": limit,
                "offset": offset,
                "active": "true",
                "closed": "false",
                "tag_slug": "weather",
            },
            timeout=30,
        )
        response.raise_for_status()
        batch = response.json()
        if not batch:
            break
        events.extend(batch)
        if len(batch) < limit:
            break
    return events


def discover_weather_contracts(
    target_dates: Optional[Iterable[date]] = None,
    include_skipped: bool = True,
) -> list[WeatherContractDiscovery]:
    events = fetch_weather_events()
    return discover_contracts_from_events(events, target_dates, include_skipped)


def discover_contracts_from_events(
    events: Iterable[dict],
    target_dates: Optional[Iterable[date]] = None,
    include_skipped: bool = True,
) -> list[WeatherContractDiscovery]:
    targets = {d.isoformat() for d in target_dates} if target_dates else None
    results: list[WeatherContractDiscovery] = []

    for event in events:
        result = validate_weather_event(event)
        if targets and result.contract.target_date not in targets:
            continue
        if result.is_valid or include_skipped:
            results.append(result)

    return results


def configured_weather_contracts(target_dates: Iterable[date]) -> list[WeatherContract]:
    contracts = []
    for city_slug, cfg in enabled_daily_high_contracts().items():
        for target in target_dates:
            event_slug = build_daily_high_slug(city_slug, target)
            contracts.append(
                WeatherContract(
                    event_slug=event_slug,
                    event_id=None,
                    target_date=target.isoformat(),
                    market_type="daily_temperature",
                    measure="high",
                    unit="F",
                    city_slug=city_slug,
                    display_city=cfg["display_city"],
                    resolution_source_kind="wunderground",
                    station_id=cfg["station_id"],
                    source_url=cfg["source_url"],
                    timezone=cfg["tz"],
                    lat=float(cfg["lat"]),
                    lon=float(cfg["lon"]),
                    validation_status="configured",
                    skip_reason=None,
                )
            )
    return contracts


def validate_weather_event(event: dict) -> WeatherContractDiscovery:
    slug = event.get("slug") or event.get("_slug") or ""
    parsed = parse_daily_high_slug(slug)
    if parsed is None:
        return _skipped(slug, event, "unsupported_slug")

    city_slug, target = parsed
    cfg = DAILY_HIGH_F_CONTRACTS.get(city_slug)
    if not cfg or not cfg.get("enabled", True):
        return _skipped(slug, event, "city_not_allowlisted", city_slug=city_slug, target=target)

    text = _event_text(event)
    unit = _detect_unit(text)
    if unit != "F":
        return _skipped(
            slug,
            event,
            "unsupported_unit",
            city_slug=city_slug,
            target=target,
            cfg=cfg,
            unit=unit or "",
        )

    source_url = _source_url(event, text)
    if "wunderground.com/history/daily/" not in source_url.lower():
        return _skipped(slug, event, "unsupported_resolution_source", city_slug=city_slug, target=target, cfg=cfg)

    station_id = _station_from_source_url(source_url)
    if not station_id:
        return _skipped(slug, event, "missing_station", city_slug=city_slug, target=target, cfg=cfg)

    expected_station = cfg["station_id"].upper()
    if station_id.upper() != expected_station:
        return _skipped(
            slug,
            event,
            f"station_mismatch:{station_id}!={expected_station}",
            city_slug=city_slug,
            target=target,
            cfg=cfg,
            station_id=station_id,
            source_url=source_url,
        )

    contract = _contract(
        slug,
        event,
        city_slug,
        target,
        cfg,
        "valid",
        station_id=station_id.upper(),
        source_url=source_url,
        skip_reason=None,
    )
    return WeatherContractDiscovery(contract=contract, event=event)


def _skipped(
    slug: str,
    event: dict,
    reason: str,
    city_slug: str = "",
    target: Optional[date] = None,
    cfg: Optional[dict] = None,
    unit: str = "",
    station_id: str = "",
    source_url: str = "",
) -> WeatherContractDiscovery:
    cfg = cfg or {}
    target = target or _fallback_target_date(event)
    contract = _contract(
        slug,
        event,
        city_slug,
        target or date.min,
        cfg,
        "skipped",
        station_id=station_id or cfg.get("station_id", ""),
        source_url=source_url or cfg.get("source_url", ""),
        skip_reason=reason,
        unit=unit or "F",
    )
    return WeatherContractDiscovery(contract=contract, event=event)


def _contract(
    slug: str,
    event: dict,
    city_slug: str,
    target: date,
    cfg: dict,
    status: str,
    station_id: str,
    source_url: str,
    skip_reason: Optional[str],
    unit: str = "F",
) -> WeatherContract:
    return WeatherContract(
        event_slug=slug,
        event_id=str(event.get("id", "")) if event else None,
        target_date=target.isoformat(),
        market_type="daily_temperature",
        measure="high",
        unit=unit,
        city_slug=city_slug,
        display_city=cfg.get("display_city") or _title_from_slug(city_slug),
        resolution_source_kind="wunderground" if "wunderground.com" in source_url.lower() else "",
        station_id=station_id,
        source_url=source_url,
        timezone=cfg.get("tz", ""),
        lat=float(cfg["lat"]) if cfg.get("lat") is not None else 0.0,
        lon=float(cfg["lon"]) if cfg.get("lon") is not None else 0.0,
        validation_status=status,
        skip_reason=skip_reason,
        raw_json=json.dumps({
            "title": event.get("title"),
            "slug": event.get("slug"),
            "resolutionSource": event.get("resolutionSource"),
        }) if event else None,
    )


def _event_text(event: dict) -> str:
    chunks = [
        event.get("title") or "",
        event.get("description") or "",
        event.get("resolutionSource") or "",
    ]
    for market in event.get("markets") or []:
        chunks.extend([
            market.get("question") or "",
            market.get("groupItemTitle") or "",
            market.get("description") or "",
            market.get("resolutionSource") or "",
        ])
    return "\n".join(chunks)


def _detect_unit(text: str) -> Optional[str]:
    lower = text.lower()
    if "°f" in text or "degrees fahrenheit" in lower:
        return "F"
    if "°c" in text or "degrees celsius" in lower:
        return "C"
    return None


def _source_url(event: dict, text: str) -> str:
    source = event.get("resolutionSource") or ""
    if source:
        return source
    match = re.search(r"https://www\.wunderground\.com/history/daily/[^\s)]+", text)
    return match.group(0).rstrip(".,") if match else ""


def _station_from_source_url(source_url: str) -> str:
    if not source_url:
        return ""
    return source_url.rstrip("/").split("/")[-1].upper()


def _fallback_target_date(event: dict) -> Optional[date]:
    raw = event.get("endDate") or event.get("end_date")
    if isinstance(raw, str) and len(raw) >= 10:
        try:
            return date.fromisoformat(raw[:10])
        except ValueError:
            pass

    slug = event.get("slug") or event.get("_slug") or ""
    match = re.search(r"-on-([a-z]+)-(\d{1,2})-(\d{4})$", slug, re.IGNORECASE)
    if match:
        month = MONTHS.get(match.group(1).lower())
        if month:
            try:
                return date(int(match.group(3)), month, int(match.group(2)))
            except ValueError:
                return None
    return None


def _title_from_slug(slug: str) -> str:
    return " ".join(part.capitalize() for part in slug.split("-")) if slug else ""
