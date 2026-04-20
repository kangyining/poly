"""
Forecast recorder — fetches Open-Meteo ECMWF + GFS forecasts every hour.

Stores max temp forecasts in both °C and °F for the next 3 days.
Polymarket settles in whole °F, so we track both units.
"""

import json
import logging
from datetime import date, datetime, timedelta, timezone

import requests

from config.cities import CITIES
from data.db import Database
from data.models import ForecastRecord

log = logging.getLogger(__name__)

OPEN_METEO_URL = "https://api.open-meteo.com/v1/forecast"


def c_to_f(c: float) -> float:
    return round(c * 9 / 5 + 32, 1)


def _lead_hours(target: date, tz: str, now: datetime) -> float:
    """Hours from now until midnight local time of the target date."""
    from zoneinfo import ZoneInfo
    local_midnight = datetime(
        target.year, target.month, target.day,
        0, 0, 0, tzinfo=ZoneInfo(tz)
    )
    delta = local_midnight - now
    return max(0.0, delta.total_seconds() / 3600)


def fetch_forecast(lat: float, lon: float, tz: str) -> dict:
    params = {
        "latitude":     lat,
        "longitude":    lon,
        "daily":        "temperature_2m_max",
        "models":       "ecmwf_ifs025,gfs_seamless",
        "forecast_days": 3,
        "timezone":     tz,
    }
    r = requests.get(OPEN_METEO_URL, params=params, timeout=15)
    r.raise_for_status()
    return r.json()


def record_forecasts(db: Database):
    """Fetch and store forecasts for all cities, for each of next 3 days."""
    now = datetime.now(timezone.utc)

    for city, cfg in CITIES.items():
        try:
            raw = fetch_forecast(cfg["lat"], cfg["lon"], cfg["tz"])
        except Exception as e:
            log.error("Forecast fetch failed for %s: %s", city, e)
            continue

        daily = raw.get("daily", {})
        dates  = daily.get("time", [])
        ecmwf  = daily.get("temperature_2m_max_ecmwf_ifs025", [])
        gfs    = daily.get("temperature_2m_max_gfs_seamless", [])

        for i, date_str in enumerate(dates):
            target = date.fromisoformat(date_str)
            ec = ecmwf[i] if i < len(ecmwf) else None
            gf = gfs[i] if i < len(gfs) else None

            spread = abs(ec - gf) if ec is not None and gf is not None else None

            rec = ForecastRecord(
                timestamp=now,
                city=city,
                target_date=date_str,
                source="open-meteo",
                temp_max_ecmwf_c=ec,
                temp_max_gfs_c=gf,
                temp_max_ecmwf_f=c_to_f(ec) if ec is not None else None,
                temp_max_gfs_f=c_to_f(gf) if gf is not None else None,
                model_spread_c=spread,
                forecast_lead_hours=_lead_hours(target, cfg["tz"], now),
                raw_json=json.dumps(raw),
            )
            db.insert_forecast(rec)
            log.debug(
                "Forecast %s %s: ECMWF=%.1f°F GFS=%.1f°F spread=%.1f°C",
                city, date_str,
                rec.temp_max_ecmwf_f or 0,
                rec.temp_max_gfs_f or 0,
                spread or 0,
            )

        log.info("Forecasts saved for %s (%d days)", city, len(dates))
