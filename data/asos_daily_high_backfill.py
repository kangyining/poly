"""
Backfill missing settlement temperatures from Iowa Mesonet ASOS historical data.

ASOS = Automated Surface Observation System (NOAA/FAA)
This is the same source that Wunderground uses for settlement.

Usage:
  python run_recorder.py --backfill-settlement-temps

Or standalone:
  python -c "from data.asos_daily_high_backfill import backfill_missing_temps; from data.db import Database; backfill_missing_temps(Database())"
"""

import csv
import io
import json
import logging
import time
from datetime import datetime
from typing import Optional

import requests

from config.weather_contracts import (
    contract_for_city,
    enabled_daily_high_contracts,
    normalize_contract_city,
)
from data.db import Database

log = logging.getLogger(__name__)

ASOS_API = "https://mesonet.agron.iastate.edu/cgi-bin/request/asos.py"
ASOS_SOURCE = "asos-iowa-mesonet"


def _parse_daily_high_asos_csv(text: str) -> tuple[Optional[float], int]:
    """Return max tmpf and valid reading count from Iowa Mesonet ASOS CSV."""
    clean = "\n".join(
        line for line in text.splitlines()
        if line.strip() and not line.startswith("#")
    )
    if not clean:
        return None, 0

    temps: list[float] = []
    for row in csv.DictReader(io.StringIO(clean)):
        raw = (row.get("tmpf") or "").strip()
        if raw in ("", "M"):
            continue
        try:
            temps.append(float(raw))
        except ValueError:
            continue

    if not temps:
        return None, 0
    return max(temps), len(temps)


def _fetch_daily_high_asos(station: str, target_date: str, retries: int = 3) -> Optional[float]:
    """
    Fetch daily high temperature from Iowa Mesonet ASOS for a specific date.

    Args:
        station: ICAO code (e.g., "KLGA")
        target_date: YYYY-MM-DD format
        retries: number of retry attempts

    Returns:
        Daily high temperature in Fahrenheit, or None if fetch fails
    """
    for attempt in range(retries):
        try:
            datetime.fromisoformat(target_date)
            # ISO 8601 format with Z (UTC)
            sts = f"{target_date}T00:00:00Z"
            ets = f"{target_date}T23:59:59Z"

            # Fetch from Iowa Mesonet ASOS API (CSV format)
            params = {
                "station": station,
                "data": "tmpf",           # temperature in Fahrenheit
                "sts": sts,
                "ets": ets,
                "format": "csv",          # CSV is simpler to parse
            }

            r = requests.get(ASOS_API, params=params, timeout=15)
            r.raise_for_status()
            daily_high, observation_count = _parse_daily_high_asos_csv(r.text)
            if daily_high is not None:
                log.debug("ASOS %s %s: high=%.1f°F (from %d readings)",
                         station, target_date, daily_high, observation_count)
                return daily_high

            log.debug("No temperature readings for %s on %s", station, target_date)
            return None

        except requests.exceptions.Timeout:
            log.warning("ASOS timeout for %s %s (attempt %d/%d)",
                       station, target_date, attempt + 1, retries)
            if attempt < retries - 1:
                time.sleep(2 ** attempt)
        except Exception as e:
            log.warning("ASOS fetch failed for %s %s (attempt %d/%d): %s",
                       station, target_date, attempt + 1, retries, e)
            if attempt < retries - 1:
                time.sleep(2 ** attempt)

    log.warning("Could not fetch ASOS daily high for %s on %s after %d retries",
               station, target_date, retries)
    return None


def _contract_filter(city: str | None) -> dict[str, dict]:
    if city:
        slug, cfg = contract_for_city(city)
        if not cfg:
            log.error("No enabled daily-high contract for city %r", city)
            return {}
        return {slug: cfg}
    return enabled_daily_high_contracts()


def _row_station(row, cfg: dict) -> str:
    return (
        row["station_id"]
        or row["wunderground_station"]
        or row["settlement_temp_station"]
        or cfg["station_id"]
    )


def backfill_missing_temps(db: Database, city: str | None = None) -> int:
    """
    Backfill missing settlement temperatures using ASOS data.

    Args:
        db: Database instance
        city: Optional display city or slug; default covers all enabled contracts

    Returns:
        Number of records updated
    """
    updated = 0
    failed = 0
    contracts = _contract_filter(city)
    if not contracts:
        return 0

    for _, cfg in contracts.items():
        city_name = cfg["display_city"]
        rows = db.conn.execute(
            """
            SELECT city, target_date, wunderground_station, station_id,
                   settlement_temp_station
            FROM settlement_records
            WHERE city = ?
              AND (settlement_temp_f IS NULL OR settlement_temp_source IS NULL)
            ORDER BY city, target_date
            """,
            (city_name,),
        ).fetchall()

        if not rows:
            log.info("No missing settlement temperatures for %s", city_name)
            continue

        log.info("Found %d missing temperatures for %s, fetching from ASOS...",
                 len(rows), city_name)

        for row in rows:
            row_city = row["city"]
            target_date = row["target_date"]
            station = _row_station(row, cfg)

            if not station:
                log.warning("No ASOS station configured for %s, skipping %s", row_city, target_date)
                failed += 1
                continue

            temp = _fetch_daily_high_asos(station, target_date)

            if temp is not None:
                raw_summary = json.dumps({
                    "source": ASOS_SOURCE,
                    "station": station,
                    "target_date": target_date,
                    "daily_high_f": temp,
                    "source_api": ASOS_API,
                }, sort_keys=True)
                db.conn.execute(
                    """
                    UPDATE settlement_records
                    SET settlement_temp_f = ?,
                        settlement_value = ?,
                        settlement_unit = 'F',
                        settlement_temp_source = ?,
                        settlement_temp_station = ?,
                        settlement_temp_raw_json = ?,
                        station_id = COALESCE(station_id, ?),
                        wunderground_station = COALESCE(wunderground_station, ?)
                    WHERE city = ? AND target_date = ?
                    """,
                    (temp, temp, ASOS_SOURCE, station, raw_summary, station, station,
                     row_city, target_date),
                )
                db.conn.commit()
                log.info("Updated %s %s: settlement_temp_f = %.1f°F (from %s)",
                         row_city, target_date, temp, station)
                updated += 1
            else:
                log.debug("Could not fetch temp for %s %s", row_city, target_date)
                failed += 1

            # Gentle rate limiting
            time.sleep(0.5)

    city_label = normalize_contract_city(city) if city else "all enabled cities"
    log.info(
        "ASOS backfill complete for %s: %d updated, %d failed",
        city_label, updated, failed
    )
    return updated
