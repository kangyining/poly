"""
METAR recorder — fetches hourly observations for settlement stations.

Stations (Phase 0 verified):
  Chicago → KORD  (O'Hare, matches Polymarket Wunderground settlement)
  Dallas  → KDAL  (Love Field, NOT KDFW/DFW)
  NYC     → KLGA  (LaGuardia, NOT KJFK/JFK)
"""

import json
import logging
from datetime import datetime, timezone

import requests

from config.cities import CITIES
from data.db import Database
from data.models import METARRecord

log = logging.getLogger(__name__)

METAR_URL = "https://aviationweather.gov/api/data/metar"


def c_to_f(c: float) -> float:
    return round(c * 9 / 5 + 32, 1)


def fetch_metars(icao_list: list[str]) -> list[dict]:
    ids = ",".join(icao_list)
    r = requests.get(METAR_URL, params={"ids": ids, "format": "json"}, timeout=15)
    r.raise_for_status()
    return r.json()


def parse_metar(raw: dict) -> METARRecord | None:
    """Parse one METAR JSON object from aviationweather.gov."""
    icao = raw.get("icaoId") or raw.get("stationId", "")
    temp_c = raw.get("temp")
    if temp_c is None:
        log.warning("No temp in METAR for %s", icao)
        return None

    # Map ICAO to city name
    city = next(
        (name for name, cfg in CITIES.items() if cfg["icao"] == icao),
        icao
    )

    # Parse observation time
    obs_time_raw = raw.get("obsTime") or raw.get("reportTime")
    try:
        obs_time = datetime.fromtimestamp(int(obs_time_raw), tz=timezone.utc)
    except (TypeError, ValueError):
        obs_time = datetime.now(timezone.utc)

    return METARRecord(
        timestamp=datetime.now(timezone.utc),
        icao=icao,
        city=city,
        temp_c=float(temp_c),
        temp_f=c_to_f(float(temp_c)),
        dewpoint_c=float(raw["dewp"]) if raw.get("dewp") is not None else None,
        obs_time=obs_time,
        raw_metar=raw.get("rawOb", ""),
    )


def record_metars(db: Database):
    """Fetch and store current METAR for all settlement stations."""
    icao_list = [cfg["icao"] for cfg in CITIES.values()]
    try:
        raw_list = fetch_metars(icao_list)
    except Exception as e:
        log.error("METAR fetch failed: %s", e)
        return

    saved = 0
    for raw in raw_list:
        rec = parse_metar(raw)
        if rec:
            db.insert_metar(rec)
            log.debug("METAR %s: %.1f°C / %.1f°F", rec.icao, rec.temp_c, rec.temp_f)
            saved += 1

    log.info("METAR: saved %d/%d records", saved, len(icao_list))
