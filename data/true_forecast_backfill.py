"""
True 24h forecast backfill — fetches what GFS actually predicted 24h in advance.

Problem this solves:
  Our existing forecast_records (source='open-meteo-historical') use gfs_seamless
  reanalysis data (lead=-1), which is an after-the-fact best estimate with ~1.7°F MAE.
  Real 24h GFS forecasts have ~3.3°F MAE — significantly different for backtesting.

Solution:
  Use Open-Meteo Historical Forecast API with:
    - model: gfs_global (single model run, not a seamless blend)
    - past_model_run: target_date - 1 day at 06:00 UTC (~26h lead before noon UTC close)

  This gives us what GFS actually forecast ~26h before market close —
  the same forecast a trader would have seen at entry time.

Stores results as source='gfs-global-24h' with forecast_lead_hours≈26.

Usage:
  python run_recorder.py --backfill-true-forecasts
  python run_recorder.py --backfill-true-forecasts --city NYC
"""

import json
import logging
import time
from datetime import date, datetime, timedelta, timezone

import requests

from config.weather_contracts import contract_for_city, enabled_daily_high_contracts
from data.weather_contracts import build_daily_high_slug
from data.db import Database
from data.models import ForecastRecord

log = logging.getLogger(__name__)

HISTORICAL_FORECAST_URL = "https://historical-forecast-api.open-meteo.com/v1/forecast"
SOURCE = "gfs-global-24h"


def _prev_day_run(target_date: str) -> str:
    """Return D-1 06:00 UTC model run time for a given target date string."""
    d = date.fromisoformat(target_date)
    prev = d - timedelta(days=1)
    return f"{prev.isoformat()}T06:00"


def fetch_true_forecast(lat: float, lon: float,
                        target_date: str) -> float | None:
    """
    Fetch gfs_global daily max temp for target_date using D-1 06:00 model run.
    Returns temperature in Fahrenheit, or None on failure.
    """
    model_run = _prev_day_run(target_date)

    for attempt in range(3):
        try:
            r = requests.get(
                HISTORICAL_FORECAST_URL,
                params={
                    "latitude":           lat,
                    "longitude":          lon,
                    "start_date":         target_date,
                    "end_date":           target_date,
                    "daily":              "temperature_2m_max",
                    "models":             "gfs_global",
                    "past_model_run":     model_run,
                    "temperature_unit":   "fahrenheit",
                    "timezone":           "UTC",
                },
                timeout=20,
            )
            r.raise_for_status()
            vals = r.json().get("daily", {}).get("temperature_2m_max", [])
            return vals[0] if vals else None
        except Exception as e:
            if attempt == 2:
                log.warning("true forecast fetch failed %s run=%s: %s",
                            target_date, model_run, e)
                return None
            time.sleep(2 ** attempt)
    return None


def backfill_true_forecasts(db: Database, city: str | None = None) -> int:
    """
    For each settlement date, fetch the true 24h GFS forecast and store it.
    Skips dates already in forecast_records with source='gfs-global-24h'.

    Args:
        db: Database instance
        city: if given, only backfill this city; otherwise all enabled contracts

    Returns:
        number of records saved
    """
    now = datetime.now(timezone.utc)
    if city:
        slug, cfg = contract_for_city(city)
        cities = {slug: cfg} if cfg else {}
        if not cfg:
            log.error("No enabled daily-high contract for city %r", city)
    else:
        cities = enabled_daily_high_contracts()
    total_saved = 0

    for city_slug, cfg in cities.items():
        city_name = cfg["display_city"]
        # All settlement dates for this city
        rows = db.conn.execute(
            """
            SELECT target_date, event_slug
            FROM settlement_records
            WHERE city = ?
            ORDER BY target_date
            """,
            (city_name,)
        ).fetchall()

        if not rows:
            log.info("%s: no settlements, skipping", city_name)
            continue

        # Already done
        existing = set(
            r[0] for r in db.conn.execute(
                "SELECT target_date FROM forecast_records "
                "WHERE city = ? AND source = ?",
                (city_name, SOURCE)
            ).fetchall()
        )

        missing = [r for r in rows if r["target_date"] not in existing]

        if not missing:
            log.info("%s: all %d dates already have true forecasts", city_name, len(rows))
            continue

        log.info("%s: fetching true 24h forecasts for %d dates", city_name, len(missing))
        saved = 0

        for row in missing:
            target_date = row["target_date"]
            gfs_f = fetch_true_forecast(cfg["lat"], cfg["lon"], target_date)

            if gfs_f is None:
                log.warning("%s %s: no data returned", city_name, target_date)
                time.sleep(0.3)
                continue

            gfs_c = round((gfs_f - 32) * 5 / 9, 1)
            model_run = _prev_day_run(target_date)
            event_slug = row["event_slug"] or build_daily_high_slug(
                city_slug, date.fromisoformat(target_date)
            )

            # Market closes noon UTC; D-1 06:00 run = ~26h lead
            lead_hours = 26.0

            rec = ForecastRecord(
                timestamp=now,
                city=city_name,
                target_date=target_date,
                source=SOURCE,
                temp_max_ecmwf_c=None,
                temp_max_gfs_c=gfs_c,
                temp_max_ecmwf_f=None,
                temp_max_gfs_f=gfs_f,
                model_spread_c=None,
                forecast_lead_hours=lead_hours,
                raw_json=json.dumps({
                    "model": "gfs_global",
                    "past_model_run": model_run,
                    "target_date": target_date,
                    "gfs_f": gfs_f,
                }),
                market_type="daily_temperature",
                measure="high",
                unit="F",
                city_slug=city_slug,
                station_id=cfg["station_id"],
                event_slug=event_slug,
                value_gfs=gfs_f,
                value_unit="F",
            )
            db.insert_forecast(rec)
            db.insert_forecast_model_run({
                "timestamp": now.isoformat(),
                "city": city_name,
                "target_date": target_date,
                "source": SOURCE,
                "model_name": "gfs_global",
                "model_run_time": model_run,
                "forecast_lead_hours": lead_hours,
                "temp_max_f": gfs_f,
                "raw_json": json.dumps({
                    "model": "gfs_global",
                    "past_model_run": model_run,
                    "target_date": target_date,
                    "gfs_f": gfs_f,
                }),
                "market_type": "daily_temperature",
                "measure": "high",
                "unit": "F",
                "city_slug": city_slug,
                "station_id": cfg["station_id"],
                "event_slug": event_slug,
                "value": gfs_f,
                "value_unit": "F",
            })
            saved += 1

            log.debug("%s %s: GFS 24h = %.1f°F (run %s)",
                      city_name, target_date, gfs_f, model_run)
            time.sleep(0.25)

        log.info("%s: saved %d true forecast records", city_name, saved)
        total_saved += saved

    log.info("True forecast backfill complete: %d records saved", total_saved)
    return total_saved
