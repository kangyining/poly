"""
Legacy compatibility view of configured US daily high-temperature contracts.

New Python data paths should use config.weather_contracts directly. This module
keeps the old CITIES shape for scripts or notebooks that still import it.
"""

from __future__ import annotations

from config.weather_contracts import enabled_daily_high_contracts


CITIES = {
    cfg["display_city"]: {
        "lat": cfg["lat"],
        "lon": cfg["lon"],
        "icao": cfg["station_id"],
        "tz": cfg["tz"],
        "polymarket_slug_city": slug,
        "wunderground_station": cfg["station_id"],
        "source_url": cfg["source_url"],
        "nws_forecast_url": None,
    }
    for slug, cfg in enabled_daily_high_contracts().items()
}
