"""Backfill historical hourly ASOS observations into metar_records."""

from __future__ import annotations

import csv
import io
import json
import logging
import time
from datetime import datetime, timezone

import requests

from config.cities import CITIES
from data.db import Database
from data.models import METARRecord

log = logging.getLogger(__name__)

ASOS_API = "https://mesonet.agron.iastate.edu/cgi-bin/request/asos.py"


def _f_to_c(value: float) -> float:
    return round((value - 32.0) * 5.0 / 9.0, 1)


def _parse_valid(value: str) -> datetime | None:
    value = value.strip()
    if not value:
        return None
    if value.endswith("Z"):
        value = value[:-1] + "+00:00"
    elif "+" not in value:
        value += "+00:00"
    try:
        return datetime.fromisoformat(value)
    except ValueError:
        try:
            return datetime.strptime(value, "%Y-%m-%d %H:%M").replace(tzinfo=timezone.utc)
        except ValueError:
            return None


def fetch_asos_observations(station: str, target_date: str, retries: int = 4) -> list[METARRecord]:
    params = [
        ("station", station),
        ("data", "tmpf"),
        ("data", "dwpf"),
        ("data", "metar"),
        ("sts", f"{target_date}T00:00:00Z"),
        ("ets", f"{target_date}T23:59:59Z"),
        ("tz", "UTC"),
        ("format", "csv"),
    ]
    response = None
    for attempt in range(retries):
        response = requests.get(ASOS_API, params=params, timeout=30)
        if response.status_code != 429:
            response.raise_for_status()
            break
        wait = 2.0 * (attempt + 1)
        log.warning("ASOS rate limited for %s %s; sleeping %.1fs", station, target_date, wait)
        time.sleep(wait)
    assert response is not None
    response.raise_for_status()
    text = "\n".join(line for line in response.text.splitlines() if line.strip() and not line.startswith("#"))
    if not text:
        return []

    city = next((name for name, cfg in CITIES.items() if cfg["icao"] == station), station)
    now = datetime.now(timezone.utc)
    records: list[METARRecord] = []
    for row in csv.DictReader(io.StringIO(text)):
        if row.get("tmpf") in (None, "", "M"):
            continue
        obs_time = _parse_valid(row.get("valid", ""))
        if obs_time is None:
            continue
        try:
            temp_f = float(row["tmpf"])
        except ValueError:
            continue
        dewpoint_c = None
        if row.get("dwpf") not in (None, "", "M"):
            try:
                dewpoint_c = _f_to_c(float(row["dwpf"]))
            except ValueError:
                dewpoint_c = None
        records.append(
            METARRecord(
                timestamp=now,
                icao=station,
                city=city,
                temp_c=_f_to_c(temp_f),
                temp_f=round(temp_f, 1),
                dewpoint_c=dewpoint_c,
                obs_time=obs_time,
                raw_metar=row.get("metar") or json.dumps(row, sort_keys=True),
            )
        )
    return records


def backfill_asos_metars(db: Database, city: str | None = None) -> int:
    cities = {city: CITIES[city]} if city else CITIES
    total = 0
    for city_name, cfg in cities.items():
        rows = db.conn.execute(
            """
            SELECT DISTINCT target_date
            FROM settlement_records
            WHERE city = ? AND winning_bin IS NOT NULL
            ORDER BY target_date
            """,
            (city_name,),
        ).fetchall()
        for row in rows:
            target_date = row["target_date"]
            existing = db.conn.execute(
                """
                SELECT COUNT(*) AS c
                FROM metar_records
                WHERE city = ? AND DATE(COALESCE(obs_time, timestamp)) = ?
                """,
                (city_name, target_date),
            ).fetchone()["c"]
            if existing:
                continue
            try:
                records = fetch_asos_observations(cfg["icao"], target_date)
            except Exception as e:
                log.warning("ASOS METAR backfill failed for %s %s: %s", city_name, target_date, e)
                time.sleep(0.5)
                continue
            for record in records:
                db.insert_metar(record)
            total += len(records)
            log.info("ASOS METAR %s %s: saved %d observations", city_name, target_date, len(records))
            time.sleep(1.0)
    log.info("ASOS METAR backfill complete: %d observations saved", total)
    return total
