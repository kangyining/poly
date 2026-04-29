"""
Historical forecast backfill — P1 data source addition.

Uses Open-Meteo Previous Runs (Historical Forecast) API to fetch what
ECMWF/GFS predicted for each past date we have a settlement record for.

Endpoint: historical-forecast-api.open-meteo.com/v1/forecast
- Returns daily temperature_2m_max for any past date range
- Free, no API key required
- Data available from ~2021 onwards

Batches requests by city (one call per city covers all settlement dates).
Stores results in forecast_records with source='open-meteo-historical'.
"""

import json
import logging
import time
from datetime import date, datetime, timezone

import requests

from config.cities import CITIES
from data.db import Database
from data.models import ForecastRecord

log = logging.getLogger(__name__)

HISTORICAL_FORECAST_URL = "https://historical-forecast-api.open-meteo.com/v1/forecast"


def c_to_f(c: float) -> float:
    return round(c * 9 / 5 + 32, 1)


def fetch_historical_forecasts(lat: float, lon: float,
                                start: date, end: date) -> dict:
    """Fetch historical model output for a date range."""
    params = {
        "latitude":         lat,
        "longitude":        lon,
        "start_date":       start.isoformat(),
        "end_date":         end.isoformat(),
        "daily":            "temperature_2m_max",
        "models":           "ecmwf_ifs025,gfs_seamless",
        "temperature_unit": "fahrenheit",
        "timezone":         "UTC",
    }
    r = requests.get(HISTORICAL_FORECAST_URL, params=params, timeout=30)
    r.raise_for_status()
    return r.json()


def backfill_historical_forecasts(db: Database):
    """
    For each city, find all settlement dates that lack a historical forecast
    record, then fetch and store them in one batched API call per city.
    """
    now = datetime.now(timezone.utc)
    total_saved = 0

    for city, cfg in CITIES.items():
        # Find settlement dates for this city
        rows = db.conn.execute(
            "SELECT target_date FROM settlement_records WHERE city = ? ORDER BY target_date",
            (city,)
        ).fetchall()

        if not rows:
            log.info("%s: no settlements found, skipping", city)
            continue

        # Find which dates already have a historical forecast record
        existing = set(
            r[0] for r in db.conn.execute(
                "SELECT target_date FROM forecast_records "
                "WHERE city = ? AND source = 'open-meteo-historical'",
                (city,)
            ).fetchall()
        )

        missing_dates = [
            date.fromisoformat(r[0]) for r in rows
            if r[0] not in existing
        ]

        if not missing_dates:
            log.info("%s: all %d dates already backfilled", city, len(rows))
            continue

        start = min(missing_dates)
        end   = max(missing_dates)
        log.info("%s: fetching %d missing dates (%s to %s)",
                 city, len(missing_dates), start, end)

        try:
            raw = fetch_historical_forecasts(cfg["lat"], cfg["lon"], start, end)
        except Exception as e:
            log.error("%s: historical fetch failed: %s", city, e)
            continue

        daily  = raw.get("daily", {})
        dates  = daily.get("time", [])
        ecmwf  = daily.get("temperature_2m_max_ecmwf_ifs025", [])
        gfs    = daily.get("temperature_2m_max_gfs_seamless", [])

        # Build lookup: date_str -> (ecmwf_f, gfs_f)
        forecast_lookup = {}
        for i, d_str in enumerate(dates):
            ec_f = ecmwf[i] if i < len(ecmwf) and ecmwf[i] is not None else None
            gf_f = gfs[i] if i < len(gfs) and gfs[i] is not None else None
            forecast_lookup[d_str] = (ec_f, gf_f)

        saved = 0
        for d in missing_dates:
            d_str = d.isoformat()
            if d_str not in forecast_lookup:
                log.debug("%s %s: not in API response", city, d_str)
                continue

            ec_f, gf_f = forecast_lookup[d_str]
            spread = abs(ec_f - gf_f) if ec_f is not None and gf_f is not None else None

            # Convert F back to C for storage (schema stores both)
            ec_c = round((ec_f - 32) * 5 / 9, 1) if ec_f is not None else None
            gf_c = round((gf_f - 32) * 5 / 9, 1) if gf_f is not None else None
            spread_c = abs(ec_c - gf_c) if ec_c is not None and gf_c is not None else None

            rec = ForecastRecord(
                timestamp=now,
                city=city,
                target_date=d_str,
                source="open-meteo-historical",
                temp_max_ecmwf_c=ec_c,
                temp_max_gfs_c=gf_c,
                temp_max_ecmwf_f=ec_f,
                temp_max_gfs_f=gf_f,
                model_spread_c=spread_c,
                forecast_lead_hours=-1.0,   # historical: target already passed
                raw_json=json.dumps({"date": d_str, "ecmwf_f": ec_f, "gfs_f": gf_f}),
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
                    "city": city,
                    "target_date": d_str,
                    "source": "open-meteo-historical",
                    "model_name": model_name,
                    "model_run_time": None,
                    "forecast_lead_hours": -1.0,
                    "temp_max_f": temp_f,
                    "raw_json": json.dumps({"date": d_str, "model": model_name, "temp_max_f": temp_f}),
                })
            saved += 1

        total_saved += saved
        log.info("%s: saved %d historical forecast records", city, saved)
        time.sleep(0.3)   # gentle rate limiting

    log.info("Historical forecast backfill complete: %d records saved", total_saved)
    return total_saved
