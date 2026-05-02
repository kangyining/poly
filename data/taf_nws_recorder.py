"""
TAF + NWS forecast recorder — P2 and P3 data sources.

TAF  (Terminal Aerodrome Forecast): 24-30hr airport forecast, updated 4x/day.
     Same aviationweather.gov API as METAR. Stores raw TAF for feature extraction.

NWS  (National Weather Service): official US 7-day forecast by grid point.
     Free, no API key. Provides daytime high temperatures per day.
     Source: api.weather.gov — forecast URLs are resolved from station coords.
"""

import logging
import time
from datetime import date, datetime, timedelta, timezone

import requests

from config.weather_contracts import (
    contract_for_station,
    enabled_daily_high_contracts,
    enabled_station_ids,
)
from data.db import Database

log = logging.getLogger(__name__)

TAF_URL = "https://aviationweather.gov/api/data/taf"
NWS_HEADERS = {"User-Agent": "poly-weather-bot/1.0 (contact: admin@localhost)"}
_NWS_FORECAST_URL_CACHE: dict[str, str] = {}


# ── TAF ──────────────────────────────────────────────────────────────────────

def _parse_unix(ts) -> datetime | None:
    try:
        return datetime.fromtimestamp(int(ts), tz=timezone.utc)
    except (TypeError, ValueError):
        return None


def record_tafs(db: Database):
    """Fetch and store current TAF for all settlement stations."""
    icao_list = enabled_station_ids()
    ids = ",".join(icao_list)

    try:
        r = requests.get(TAF_URL, params={"ids": ids, "format": "json"}, timeout=15)
        r.raise_for_status()
        data = r.json()
    except Exception as e:
        log.error("TAF fetch failed: %s", e)
        return

    saved = 0

    for taf in data:
        icao = taf.get("icaoId", "")
        _, cfg = contract_for_station(icao)
        city = cfg["display_city"] if cfg else icao

        issue_time  = _parse_unix(taf.get("issueTime") or taf.get("bulletinTime"))
        valid_from  = _parse_unix(taf.get("validTimeFrom"))
        valid_to    = _parse_unix(taf.get("validTimeTo"))
        raw_taf     = taf.get("rawTAF", "")

        try:
            db.insert_taf(icao, city, issue_time, valid_from, valid_to, raw_taf)
            saved += 1
            log.debug("TAF %s: valid %s - %s", icao, valid_from, valid_to)
        except Exception as e:
            log.warning("TAF insert failed %s: %s", icao, e)

    log.info("TAF: saved %d/%d records", saved, len(icao_list))


# ── NWS ──────────────────────────────────────────────────────────────────────

def _lead_hours_to_noon(target: date, now: datetime) -> float:
    """Hours from now until noon UTC on target date (market close)."""
    noon_utc = datetime(target.year, target.month, target.day, 12, 0, 0,
                        tzinfo=timezone.utc)
    delta = noon_utc - now
    return max(0.0, delta.total_seconds() / 3600)


def _resolve_nws_forecast_url(station_id: str, lat: float, lon: float) -> str | None:
    cached = _NWS_FORECAST_URL_CACHE.get(station_id)
    if cached:
        return cached
    response = requests.get(
        f"https://api.weather.gov/points/{lat:.4f},{lon:.4f}",
        headers=NWS_HEADERS,
        timeout=30,
    )
    response.raise_for_status()
    url = response.json().get("properties", {}).get("forecast")
    if url:
        _NWS_FORECAST_URL_CACHE[station_id] = url
    return url


def record_nws_forecasts(db: Database):
    """Fetch and store NWS daytime high forecasts for the next 7 days."""
    now = datetime.now(timezone.utc)
    today = now.date()
    saved = 0

    for _, cfg in enabled_daily_high_contracts().items():
        city = cfg["display_city"]
        station_id = cfg["station_id"]

        try:
            url = _resolve_nws_forecast_url(station_id, float(cfg["lat"]), float(cfg["lon"]))
            if not url:
                log.debug("%s: no NWS forecast URL resolved", city)
                continue
            r = requests.get(url, headers=NWS_HEADERS, timeout=30)
            r.raise_for_status()
            periods = r.json()["properties"]["periods"]
        except Exception as e:
            log.warning("NWS fetch failed for %s: %s", city, e)
            time.sleep(1)
            continue

        for period in periods:
            # Only store daytime periods (daily highs)
            if not period.get("isDaytime"):
                continue

            temp_f = period.get("temperature")
            if temp_f is None:
                continue

            # Parse the start time to get the target date
            start_iso = period.get("startTime", "")
            try:
                start_dt = datetime.fromisoformat(start_iso)
                target = start_dt.date()
            except ValueError:
                continue

            # Only store next 7 days
            if target < today or target > today + timedelta(days=7):
                continue

            lead = _lead_hours_to_noon(target, now)

            try:
                db.insert_nws_forecast(
                    city=city,
                    target_date=target.isoformat(),
                    period_name=period.get("name", ""),
                    is_daytime=True,
                    temp_f=float(temp_f),
                    short_forecast=period.get("shortForecast", ""),
                    lead_hours=lead,
                )
                saved += 1
                log.debug("NWS %s %s: %s -> %.0f°F (%s)",
                          city, target, period.get("name"), temp_f,
                          period.get("shortForecast", ""))
            except Exception as e:
                log.warning("NWS insert failed %s %s: %s", city, target, e)

        log.info("NWS forecasts saved for %s", city)
        time.sleep(0.5)   # NWS rate limiting — be polite

    log.info("NWS forecast recorder: saved %d records", saved)
