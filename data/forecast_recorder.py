"""
Forecast recorder — fetches Open-Meteo ECMWF + GFS forecasts every hour.

V1 records configured US Fahrenheit daily-high contracts. It keeps the legacy
Fahrenheit columns for backtests and also writes generic value/unit metadata.
"""

import json
import logging
from datetime import date, datetime, timedelta, timezone

import requests

from data.db import Database
from data.models import ForecastRecord
from data.weather_contracts import configured_weather_contracts

log = logging.getLogger(__name__)

OPEN_METEO_URL = "https://api.open-meteo.com/v1/forecast"


def c_to_f(c: float) -> float:
    return round(c * 9 / 5 + 32, 1)


def f_to_c(f: float) -> float:
    return round((f - 32) * 5 / 9, 1)


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
        "temperature_unit": "fahrenheit",
        "forecast_days": 3,
        "timezone":     tz,
    }
    r = requests.get(OPEN_METEO_URL, params=params, timeout=15)
    r.raise_for_status()
    return r.json()


def record_forecasts(db: Database):
    """Fetch and store forecasts for configured contracts, for each of next 3 days."""
    now = datetime.now(timezone.utc)
    target_dates = [now.date() + timedelta(days=offset) for offset in range(3)]
    contracts = configured_weather_contracts(target_dates)

    for contract in contracts:
        db.upsert_weather_contract(contract)
        try:
            raw = fetch_forecast(contract.lat, contract.lon, contract.timezone)
        except Exception as e:
            log.error("Forecast fetch failed for %s: %s", contract.display_city, e)
            continue

        daily = raw.get("daily", {})
        dates  = daily.get("time", [])
        ecmwf_f  = daily.get("temperature_2m_max_ecmwf_ifs025", [])
        gfs_f    = daily.get("temperature_2m_max_gfs_seamless", [])

        for i, date_str in enumerate(dates):
            if date_str != contract.target_date:
                continue

            target = date.fromisoformat(date_str)
            ec_f = ecmwf_f[i] if i < len(ecmwf_f) else None
            gf_f = gfs_f[i] if i < len(gfs_f) else None
            ec_c = f_to_c(ec_f) if ec_f is not None else None
            gf_c = f_to_c(gf_f) if gf_f is not None else None

            spread_c = abs(ec_c - gf_c) if ec_c is not None and gf_c is not None else None

            rec = ForecastRecord(
                timestamp=now,
                city=contract.display_city,
                target_date=date_str,
                source="open-meteo",
                temp_max_ecmwf_c=ec_c,
                temp_max_gfs_c=gf_c,
                temp_max_ecmwf_f=ec_f,
                temp_max_gfs_f=gf_f,
                model_spread_c=spread_c,
                forecast_lead_hours=_lead_hours(target, contract.timezone, now),
                raw_json=json.dumps(raw),
                market_type=contract.market_type,
                measure=contract.measure,
                unit=contract.unit,
                city_slug=contract.city_slug,
                station_id=contract.station_id,
                event_slug=contract.event_slug,
                value_ecmwf=ec_f,
                value_gfs=gf_f,
                value_unit=contract.unit,
            )
            db.insert_forecast(rec)
            for model_name, temp_f in (
                ("ecmwf_ifs025", ec_f),
                ("gfs_seamless", gf_f),
            ):
                if temp_f is None:
                    continue
                db.insert_forecast_model_run({
                    "timestamp": now.isoformat(),
                    "city": contract.display_city,
                    "target_date": date_str,
                    "source": "open-meteo",
                    "model_name": model_name,
                    "model_run_time": None,
                    "forecast_lead_hours": rec.forecast_lead_hours,
                    "temp_max_f": temp_f,
                    "raw_json": json.dumps({"source": "open-meteo", "model": model_name, "target_date": date_str, "temp_max_f": temp_f}),
                    "market_type": contract.market_type,
                    "measure": contract.measure,
                    "unit": contract.unit,
                    "city_slug": contract.city_slug,
                    "station_id": contract.station_id,
                    "event_slug": contract.event_slug,
                    "value": temp_f,
                    "value_unit": contract.unit,
                })
            log.debug(
                "Forecast %s %s: ECMWF=%.1f°F GFS=%.1f°F spread=%.1f°C",
                contract.display_city, date_str,
                rec.temp_max_ecmwf_f or 0,
                rec.temp_max_gfs_f or 0,
                spread_c or 0,
            )

        log.info("Forecasts saved for %s %s", contract.display_city, contract.target_date)
