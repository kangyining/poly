# config/cities.py
# City configuration for Polymarket weather bot
#
# IMPORTANT — settlement station corrections (verified Phase 0, 2026-04-06):
#   Dallas: Polymarket settles on KDAL (Love Field), NOT KDFW (DFW Airport)
#   NYC:    Polymarket settles on KLGA (LaGuardia),  NOT KJFK (JFK)
#   Chicago: KORD confirmed correct.
#
# Resolution source: Wunderground historical data (whole degrees Fahrenheit)
# Wunderground URLs:
#   Chicago: https://www.wunderground.com/history/daily/us/il/chicago/KORD
#   Dallas:  https://www.wunderground.com/history/daily/us/tx/dallas/KDAL
#   NYC:     https://www.wunderground.com/history/daily/us/ny/new-york-city/KLGA

CITIES = {
    # ── Phase 0 verified cities ──
    "Chicago": {
        "lat": 41.88,
        "lon": -87.63,
        "icao": "KORD",            # O'Hare — matches Polymarket settlement ✅
        "tz": "America/Chicago",
        "polymarket_slug_city": "chicago",
        "wunderground_station": "KORD",
        "nws_forecast_url": "https://api.weather.gov/gridpoints/LOT/76,73/forecast",
    },
    "Dallas": {
        "lat": 32.84,              # Love Field coords (not DFW)
        "lon": -96.85,
        "icao": "KDAL",            # Love Field — Polymarket settles here ✅
        "icao_note": "NOT KDFW — Polymarket uses Love Field (KDAL)",
        "tz": "America/Chicago",
        "polymarket_slug_city": "dallas",
        "wunderground_station": "KDAL",
        "nws_forecast_url": "https://api.weather.gov/gridpoints/FWD/87,107/forecast",
    },
    "NYC": {
        "lat": 40.78,              # LaGuardia coords (not JFK)
        "lon": -73.87,
        "icao": "KLGA",            # LaGuardia — Polymarket settles here ✅
        "icao_note": "NOT KJFK — Polymarket uses LaGuardia (KLGA)",
        "tz": "America/New_York",
        "polymarket_slug_city": "nyc",
        "wunderground_station": "KLGA",
        "nws_forecast_url": "https://api.weather.gov/gridpoints/OKX/37,39/forecast",
    },

    # ── Tier 1: Priority cities (to be verified in Phase 0 expansion) ──
    # "London":     {"lat": 51.51, "lon": -0.13,  "icao": "EGLL", "tz": "Europe/London"},
    # "Seoul":      {"lat": 37.57, "lon": 126.98, "icao": "RKSI", "tz": "Asia/Seoul"},
    # "Toronto":    {"lat": 43.65, "lon": -79.38, "icao": "CYYZ", "tz": "America/Toronto"},

    # ── Tier 2: To verify settlement station before enabling ──
    # "Ankara":     {"lat": 39.93, "lon": 32.86,  "icao": "LTAC", "tz": "Europe/Istanbul"},
    # "Miami":      {"lat": 25.76, "lon": -80.19, "icao": "KMIA", "tz": "America/New_York"},
    # "Atlanta":    {"lat": 33.75, "lon": -84.39, "icao": "KATL", "tz": "America/New_York"},
    # "Tokyo":      {"lat": 35.68, "lon": 139.69, "icao": "RJTT", "tz": "Asia/Tokyo"},
    # "Paris":      {"lat": 48.86, "lon": 2.35,   "icao": "LFPG", "tz": "Europe/Paris"},
    # "Istanbul":   {"lat": 41.01, "lon": 28.98,  "icao": "LTFM", "tz": "Europe/Istanbul"},
}
