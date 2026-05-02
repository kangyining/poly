"""Backfill historical hourly ASOS observations into metar_records."""

from __future__ import annotations

import csv
import io
import json
import logging
import time
from datetime import datetime, timezone

import requests

from config.weather_contracts import (
    contract_for_city,
    contract_for_station,
    enabled_daily_high_contracts,
)
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


def fetch_asos_observations(
    station: str,
    target_date: str,
    retries: int = 4,
    city_name: str | None = None,
) -> list[METARRecord]:
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

    if city_name is None:
        _, cfg = contract_for_station(station)
        city_name = cfg["display_city"] if cfg else station
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
                city=city_name,
                temp_c=_f_to_c(temp_f),
                temp_f=round(temp_f, 1),
                dewpoint_c=dewpoint_c,
                obs_time=obs_time,
                raw_metar=row.get("metar") or json.dumps(row, sort_keys=True),
            )
        )
    return records


def _contract_filter(city: str | None) -> dict[str, dict]:
    if city:
        slug, cfg = contract_for_city(city)
        if not cfg:
            log.error("No enabled daily-high contract for city %r", city)
            return {}
        return {slug: cfg}
    return enabled_daily_high_contracts()


def backfill_asos_metars(db: Database, city: str | None = None) -> int:
    total = 0
    for _, cfg in _contract_filter(city).items():
        city_name = cfg["display_city"]
        rows = db.conn.execute(
            """
            SELECT DISTINCT target_date,
                   COALESCE(station_id, settlement_temp_station, wunderground_station) AS station_id
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
            station = row["station_id"] or cfg["station_id"]
            try:
                records = fetch_asos_observations(station, target_date, city_name=city_name)
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
