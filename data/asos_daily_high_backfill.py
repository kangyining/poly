"""
Backfill missing settlement temperatures from Iowa Mesonet ASOS historical data.

ASOS = Automated Surface Observation System (NOAA/FAA)
This is the same source that Wunderground uses for settlement.

Usage:
  python run_recorder.py --backfill-settlement-temps

Or standalone:
  python -c "from data.asos_daily_high_backfill import backfill_missing_temps; from data.db import Database; backfill_missing_temps(Database())"
"""

import logging
import time
from datetime import datetime
from typing import Optional

import requests

from data.db import Database

log = logging.getLogger(__name__)

ASOS_API = "https://mesonet.agron.iastate.edu/cgi-bin/request/asos.py"


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
            # Parse target date
            dt = datetime.fromisoformat(target_date)
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
            text = r.text

            # Parse CSV format: skip header lines (start with #), skip "M" (missing)
            temps = []
            for line in text.split('\n'):
                if line.startswith('#') or not line.strip():
                    continue
                parts = line.split(',')
                if len(parts) >= 3:
                    try:
                        tmpf_str = parts[2].strip()
                        if tmpf_str != 'M' and tmpf_str != 'tmpf':  # skip header and missing
                            tmpf = float(tmpf_str)
                            temps.append(tmpf)
                    except (ValueError, IndexError):
                        continue

            if temps:
                daily_high = max(temps)
                log.debug("ASOS %s %s: high=%.1f°F (from %d readings)",
                         station, target_date, daily_high, len(temps))
                return daily_high
            else:
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


def backfill_missing_temps(db: Database, city: str = "NYC") -> int:
    """
    Backfill missing settlement temperatures for a city using ASOS data.

    Args:
        db: Database instance
        city: City name (default: "NYC")

    Returns:
        Number of records updated
    """
    # Get all settlements missing temperatures for this city
    rows = db.conn.execute(
        """
        SELECT city, target_date, wunderground_station
        FROM settlement_records
        WHERE city = ? AND settlement_temp_f IS NULL
        ORDER BY city, target_date
        """,
        (city,)
    ).fetchall()

    if not rows:
        log.info("No missing settlement temperatures for %s", city)
        return 0

    log.info("Found %d missing temperatures for %s, fetching from ASOS...", len(rows), city)

    updated = 0
    failed = 0

    for row_city, target_date, station in rows:
        if not station:
            log.warning("No ASOS station configured for %s, skipping %s", row_city, target_date)
            failed += 1
            continue

        temp = _fetch_daily_high_asos(station, target_date)

        if temp is not None:
            db.conn.execute(
                """
                UPDATE settlement_records
                SET settlement_temp_f = ?
                WHERE city = ? AND target_date = ?
                """,
                (temp, row_city, target_date)
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

    log.info(
        "ASOS backfill complete for %s: %d updated, %d failed",
        city, updated, failed
    )
    return updated
