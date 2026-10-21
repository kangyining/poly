import json
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import Mock, patch

from config.weather_contracts import (
    enabled_daily_high_contracts,
    enabled_station_ids,
    normalize_contract_city,
)
from config.cities import CITIES
from analysis.phase1_backtest import _default_event_slug, _normalize_city_filter
from data.asos_daily_high_backfill import (
    _parse_daily_high_asos_csv,
    backfill_missing_temps,
)
from data.asos_metar_backfill import fetch_asos_observations
from data.db import Database
from data.metar_recorder import parse_metar, record_metars
from data.price_history_missing_bins_backfill import backfill_missing_bins
from data.taf_nws_recorder import record_nws_forecasts, record_tafs
from data.true_forecast_backfill import backfill_true_forecasts


EXPECTED_STATIONS = {
    "KATL", "KAUS", "KORD", "KDAL", "KBKF", "KHOU",
    "KLAX", "KMIA", "KLGA", "KSFO", "KSEA",
}


class TempDbMixin:
    def setUp(self):
        tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        tmp.close()
        self.db_path = Path(tmp.name)
        self.db = Database(self.db_path)

    def tearDown(self):
        self.db.close()
        self.db_path.unlink(missing_ok=True)

    def insert_settlement(
        self,
        city: str,
        target_date: str = "2026-05-01",
        station: str | None = None,
        event_slug: str | None = None,
    ):
        self.db.conn.execute(
            """
            INSERT INTO settlement_records (
                city, target_date, event_slug, winning_bin, wunderground_station,
                station_id, raw_outcome_prices
            ) VALUES (?,?,?,?,?,?,?)
            """,
            (city, target_date, event_slug, "70-71°F", station, station, "[]"),
        )
        self.db.conn.commit()


class ContractConfigTests(unittest.TestCase):
    def test_contract_config_returns_all_11_us_station_ids(self):
        self.assertEqual(set(enabled_station_ids()), EXPECTED_STATIONS)
        self.assertEqual(len(enabled_daily_high_contracts()), 11)

    def test_city_normalization_accepts_display_names_and_slugs(self):
        self.assertEqual(normalize_contract_city("NYC"), "NYC")
        self.assertEqual(normalize_contract_city("nyc"), "NYC")
        self.assertEqual(normalize_contract_city("Los Angeles"), "Los Angeles")
        self.assertEqual(normalize_contract_city("los-angeles"), "Los Angeles")

    def test_legacy_cities_config_exposes_all_configured_us_contracts(self):
        self.assertEqual(set(cfg["icao"] for cfg in CITIES.values()), EXPECTED_STATIONS)
        self.assertEqual(CITIES["Los Angeles"]["polymarket_slug_city"], "los-angeles")
        self.assertEqual(CITIES["Seattle"]["wunderground_station"], "KSEA")

    def test_backtest_city_filter_accepts_display_names_and_slugs(self):
        self.assertEqual(_normalize_city_filter("los-angeles"), "Los Angeles")
        self.assertEqual(_normalize_city_filter("San Francisco"), "San Francisco")
        self.assertEqual(_default_event_slug("los-angeles", "2026-05-01"),
                         "highest-temperature-in-los-angeles-on-may-1-2026")


class AsosDailyHighTests(TempDbMixin, unittest.TestCase):
    def test_asos_daily_high_parser_ignores_headers_and_missing_values(self):
        csv_text = """# station data
station,valid,tmpf
KSEA,2026-05-01 00:00,M
KSEA,2026-05-01 01:00,54.0
KSEA,2026-05-01 02:00,57.5
"""
        self.assertEqual(_parse_daily_high_asos_csv(csv_text), (57.5, 2))

    def test_settlement_temp_backfill_updates_non_legacy_cities_with_metadata(self):
        rows = [
            ("Atlanta", "KATL", 82.0),
            ("Denver", "KBKF", 73.0),
            ("Seattle", "KSEA", 61.0),
        ]
        for city, station, _ in rows:
            self.insert_settlement(city, station=station)

        temps = {station: temp for _, station, temp in rows}
        with patch(
            "data.asos_daily_high_backfill._fetch_daily_high_asos",
            side_effect=lambda station, target_date: temps[station],
        ):
            self.assertEqual(backfill_missing_temps(self.db), 3)

        saved = self.db.conn.execute(
            """
            SELECT city, settlement_temp_f, settlement_value, settlement_unit,
                   settlement_temp_source, settlement_temp_station
            FROM settlement_records
            ORDER BY city
            """
        ).fetchall()
        self.assertEqual({row["settlement_temp_station"] for row in saved}, {"KATL", "KBKF", "KSEA"})
        self.assertTrue(all(row["settlement_unit"] == "F" for row in saved))
        self.assertTrue(all(row["settlement_temp_source"] == "asos-iowa-mesonet" for row in saved))
        self.assertEqual({row["settlement_temp_f"] for row in saved}, {82.0, 73.0, 61.0})


class MetarAndForecastRecorderTests(TempDbMixin, unittest.TestCase):
    def test_live_metar_recorder_requests_all_configured_stations(self):
        with patch("data.metar_recorder.fetch_metars", return_value=[]) as fetch:
            record_metars(self.db)
        self.assertEqual(set(fetch.call_args.args[0]), EXPECTED_STATIONS)

    def test_parse_metar_maps_station_to_display_city(self):
        record = parse_metar({
            "icaoId": "KSEA",
            "temp": 10,
            "dewp": 5,
            "obsTime": 1_777_600_000,
            "rawOb": "KSEA sample",
        })
        self.assertEqual(record.city, "Seattle")
        self.assertEqual(record.temp_f, 50.0)

    @patch("data.asos_metar_backfill.requests.get")
    def test_historical_asos_metar_maps_station_to_display_city(self, get):
        response = Mock(status_code=200)
        response.text = "station,valid,tmpf,dwpf,metar\nKSEA,2026-05-01 01:00,50,45,KSEA sample\n"
        response.raise_for_status = Mock()
        get.return_value = response

        records = fetch_asos_observations("KSEA", "2026-05-01")
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0].city, "Seattle")

    @patch("data.taf_nws_recorder.requests.get")
    def test_taf_recorder_requests_all_configured_stations(self, get):
        response = Mock()
        response.raise_for_status = Mock()
        response.json.return_value = []
        get.return_value = response

        record_tafs(self.db)
        ids = get.call_args.kwargs["params"]["ids"].split(",")
        self.assertEqual(set(ids), EXPECTED_STATIONS)

    @patch("data.taf_nws_recorder._resolve_nws_forecast_url", return_value="https://api.weather.gov/grid/test")
    @patch("data.taf_nws_recorder.requests.get")
    def test_nws_recorder_resolves_forecasts_from_contract_coordinates(self, get, resolver):
        response = Mock()
        response.raise_for_status = Mock()
        response.json.return_value = {
            "properties": {
                "periods": [{
                    "isDaytime": True,
                    "temperature": 70,
                    "startTime": datetime.now(timezone.utc).isoformat(),
                    "name": "Today",
                    "shortForecast": "Sunny",
                }]
            }
        }
        get.return_value = response

        record_nws_forecasts(self.db)
        self.assertEqual(resolver.call_count, 11)

    @patch("data.true_forecast_backfill.fetch_true_forecast", return_value=66.6)
    def test_true_forecast_backfill_writes_contract_metadata_for_non_legacy_city(self, _fetch):
        self.insert_settlement(
            "Seattle",
            station="KSEA",
            event_slug="highest-temperature-in-seattle-on-may-1-2026",
        )

        self.assertEqual(backfill_true_forecasts(self.db, city="seattle"), 1)
        row = self.db.conn.execute(
            """
            SELECT city_slug, station_id, event_slug, value_gfs, value_unit
            FROM forecast_records
            WHERE city = 'Seattle' AND source = 'gfs-global-24h'
            """
        ).fetchone()
        self.assertEqual(dict(row), {
            "city_slug": "seattle",
            "station_id": "KSEA",
            "event_slug": "highest-temperature-in-seattle-on-may-1-2026",
            "value_gfs": 66.6,
            "value_unit": "F",
        })


class MissingBinsBackfillTests(TempDbMixin, unittest.TestCase):
    def test_missing_bin_retry_uses_stored_event_slug_for_non_legacy_city(self):
        event_slug = "custom-seattle-event-slug"
        self.insert_settlement("Seattle", station="KSEA", event_slug=event_slug)
        for idx in range(10):
            self.db.insert_price_history(
                "Seattle", "2026-05-01", f"{idx}°F", idx, f"yes-{idx}",
                "2026-04-30T12:00:00+00:00", 0.5, 24.0,
            )

        event = {
            "markets": [
                {
                    "id": str(idx),
                    "groupItemTitle": f"{idx}°F",
                    "clobTokenIds": json.dumps([f"yes-{idx}", f"no-{idx}"]),
                    "outcomePrices": json.dumps(["0.5", "0.5"]),
                }
                for idx in range(11)
            ]
        }
        with patch("data.price_history_missing_bins_backfill.fetch_event_by_slug", return_value=event) as fetch_event:
            with patch(
                "data.price_history_missing_bins_backfill._fetch_price_history",
                return_value=[{"t": 1_777_564_800, "p": 0.42}],
            ):
                saved = backfill_missing_bins(self.db, city="Seattle")

        self.assertEqual(fetch_event.call_args.args[0], event_slug)
        self.assertEqual(saved, 2)


if __name__ == "__main__":
    unittest.main()
