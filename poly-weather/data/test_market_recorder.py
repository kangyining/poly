import json
import tempfile
import unittest
from datetime import date
from pathlib import Path
from unittest.mock import patch

from data.db import Database
from data.market_recorder import record_market_snapshots
from data.models import WeatherContract
from data.weather_contracts import WeatherContractDiscovery


class MarketRecorderTests(unittest.TestCase):
    def test_records_only_valid_discovered_contracts_with_clob_data(self):
        with tempfile.TemporaryDirectory() as tmp:
            db = Database(Path(tmp) / "test.db")
            contract = WeatherContract(
                event_slug="highest-temperature-in-nyc-on-may-1-2026",
                event_id="event-1",
                target_date=date.today().isoformat(),
                market_type="daily_temperature",
                measure="high",
                unit="F",
                city_slug="nyc",
                display_city="NYC",
                resolution_source_kind="wunderground",
                station_id="KLGA",
                source_url="https://www.wunderground.com/history/daily/us/ny/new-york-city/KLGA",
                timezone="America/New_York",
                lat=40.7769,
                lon=-73.8740,
                validation_status="valid",
            )
            event = {
                "id": "event-1",
                "_slug": contract.event_slug,
                "closed": False,
                "markets": [
                    {
                        "id": "market-1",
                        "groupItemTitle": "53°F or below",
                        "outcomes": json.dumps(["Yes", "No"]),
                        "outcomePrices": json.dumps(["0.10", "0.90"]),
                        "clobTokenIds": json.dumps(["yes-token", "no-token"]),
                        "bestBid": 0.09,
                        "bestAsk": 0.11,
                        "volume24hr": 123,
                        "liquidityNum": 456,
                    }
                ],
            }
            skipped = WeatherContract(
                **{
                    **contract.__dict__,
                    "event_slug": "highest-temperature-in-london-on-may-1-2026",
                    "city_slug": "london",
                    "display_city": "London",
                    "validation_status": "skipped",
                    "skip_reason": "city_not_allowlisted",
                }
            )
            discoveries = [
                WeatherContractDiscovery(contract=contract, event=event),
                WeatherContractDiscovery(contract=skipped, event=None),
            ]
            book = {
                "bids": [{"price": "0.88", "size": "4"}],
                "asks": [{"price": "0.91", "size": "20"}],
            }

            with patch("data.market_recorder.discover_weather_contracts", return_value=discoveries), \
                 patch("data.market_recorder.fetch_order_book", return_value=book), \
                 patch("data.market_recorder.fetch_fee_rate", return_value=0.05):
                record_market_snapshots(db)

            snapshot = db.conn.execute("SELECT * FROM market_snapshots").fetchone()
            self.assertIsNotNone(snapshot)
            self.assertEqual(snapshot["city_slug"], "nyc")
            self.assertEqual(snapshot["station_id"], "KLGA")
            self.assertAlmostEqual(snapshot["no_vwap_10"], 0.91)
            self.assertEqual(snapshot["fee_rate"], 0.05)

            contracts = db.conn.execute(
                "SELECT event_slug, validation_status FROM weather_contracts ORDER BY event_slug"
            ).fetchall()
            self.assertEqual(len(contracts), 2)
            self.assertEqual([row["validation_status"] for row in contracts], ["skipped", "valid"])
            db.close()


if __name__ == "__main__":
    unittest.main()
