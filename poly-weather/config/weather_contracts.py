"""
Configured weather contracts for the Python data/paper pipeline.

V1 intentionally supports only allowlisted US Fahrenheit daily high-temperature
markets. Rust live trading still uses its existing CLI/config path.
"""

from __future__ import annotations

import re


DAILY_HIGH_F_CONTRACTS = {
    "atlanta": {
        "display_city": "Atlanta",
        "station_id": "KATL",
        "lat": 33.6407,
        "lon": -84.4277,
        "tz": "America/New_York",
        "source_url": "https://www.wunderground.com/history/daily/us/ga/atlanta/KATL",
        "enabled": True,
    },
    "austin": {
        "display_city": "Austin",
        "station_id": "KAUS",
        "lat": 30.1975,
        "lon": -97.6664,
        "tz": "America/Chicago",
        "source_url": "https://www.wunderground.com/history/daily/us/tx/austin/KAUS",
        "enabled": True,
    },
    "chicago": {
        "display_city": "Chicago",
        "station_id": "KORD",
        "lat": 41.9742,
        "lon": -87.9073,
        "tz": "America/Chicago",
        "source_url": "https://www.wunderground.com/history/daily/us/il/chicago/KORD",
        "enabled": True,
    },
    "dallas": {
        "display_city": "Dallas",
        "station_id": "KDAL",
        "lat": 32.8471,
        "lon": -96.8518,
        "tz": "America/Chicago",
        "source_url": "https://www.wunderground.com/history/daily/us/tx/dallas/KDAL",
        "enabled": True,
    },
    "denver": {
        "display_city": "Denver",
        "station_id": "KBKF",
        "lat": 39.7017,
        "lon": -104.7517,
        "tz": "America/Denver",
        "source_url": "https://www.wunderground.com/history/daily/us/co/aurora/KBKF",
        "enabled": True,
    },
    "houston": {
        "display_city": "Houston",
        "station_id": "KHOU",
        "lat": 29.6454,
        "lon": -95.2789,
        "tz": "America/Chicago",
        "source_url": "https://www.wunderground.com/history/daily/us/tx/houston/KHOU",
        "enabled": True,
    },
    "los-angeles": {
        "display_city": "Los Angeles",
        "station_id": "KLAX",
        "lat": 33.9425,
        "lon": -118.4081,
        "tz": "America/Los_Angeles",
        "source_url": "https://www.wunderground.com/history/daily/us/ca/los-angeles/KLAX",
        "enabled": True,
    },
    "miami": {
        "display_city": "Miami",
        "station_id": "KMIA",
        "lat": 25.7959,
        "lon": -80.2870,
        "tz": "America/New_York",
        "source_url": "https://www.wunderground.com/history/daily/us/fl/miami/KMIA",
        "enabled": True,
    },
    "nyc": {
        "display_city": "NYC",
        "station_id": "KLGA",
        "lat": 40.7769,
        "lon": -73.8740,
        "tz": "America/New_York",
        "source_url": "https://www.wunderground.com/history/daily/us/ny/new-york-city/KLGA",
        "enabled": True,
    },
    "san-francisco": {
        "display_city": "San Francisco",
        "station_id": "KSFO",
        "lat": 37.6213,
        "lon": -122.3790,
        "tz": "America/Los_Angeles",
        "source_url": "https://www.wunderground.com/history/daily/us/ca/san-francisco/KSFO",
        "enabled": True,
    },
    "seattle": {
        "display_city": "Seattle",
        "station_id": "KSEA",
        "lat": 47.4502,
        "lon": -122.3088,
        "tz": "America/Los_Angeles",
        "source_url": "https://www.wunderground.com/history/daily/us/wa/seatac/KSEA",
        "enabled": True,
    },
}


def enabled_daily_high_contracts() -> dict[str, dict]:
    return {
        slug: cfg
        for slug, cfg in DAILY_HIGH_F_CONTRACTS.items()
        if cfg.get("enabled", True)
    }


def contract_for_display_city(city: str) -> tuple[str, dict] | tuple[None, None]:
    for slug, cfg in DAILY_HIGH_F_CONTRACTS.items():
        if cfg["display_city"].lower() == city.lower():
            return slug, cfg
    return None, None


def _normalize_key(value: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", value.strip().lower()).strip("-")


def contract_for_city(value: str) -> tuple[str, dict] | tuple[None, None]:
    """Find an enabled contract by display city, slug, or case-insensitive alias."""
    if not value:
        return None, None

    key = _normalize_key(value)
    for slug, cfg in enabled_daily_high_contracts().items():
        if key == slug or key == _normalize_key(cfg["display_city"]):
            return slug, cfg
    return None, None


def contract_for_station(station_id: str) -> tuple[str, dict] | tuple[None, None]:
    """Find an enabled contract by ICAO/ASOS station id."""
    station = (station_id or "").strip().upper()
    if not station:
        return None, None
    for slug, cfg in enabled_daily_high_contracts().items():
        if cfg["station_id"].upper() == station:
            return slug, cfg
    return None, None


def normalize_contract_city(value: str) -> str | None:
    """Return the canonical display city for a CLI/user city string."""
    _, cfg = contract_for_city(value)
    return cfg["display_city"] if cfg else None


def enabled_station_ids() -> list[str]:
    return [cfg["station_id"] for cfg in enabled_daily_high_contracts().values()]
