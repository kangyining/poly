import importlib.util
from pathlib import Path
import tempfile
import unittest
import sys
from unittest.mock import patch

from fastapi.testclient import TestClient

API_PATH = Path(__file__).resolve().parents[1] / "main.py"
ROOT = Path(__file__).resolve().parents[4]
sys_path = str(ROOT / "tools")
sys.path.insert(0, sys_path)
from review_demo import seed_fixture
spec = importlib.util.spec_from_file_location("dashboard_api", API_PATH)
api = importlib.util.module_from_spec(spec)
spec.loader.exec_module(api)


class BacktestIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.db = patch.object(api, "DB_PATH", root / "data.db")
        self.runs = patch.object(api, "BACKTEST_DIR", root / "runs")
        self.db.start()
        self.runs.start()
        self.addCleanup(self.db.stop)
        self.addCleanup(self.runs.stop)
        self.client = TestClient(api.app)

    def test_nonempty_synthetic_trades_are_saved_and_read_back(self):
        seed_fixture(api.DB_PATH)
        result = self.client.post("/api/backtest/run", params={
            "strategy": "base_no_ev", "city": "NYC",
            "date_from": "2026-04-02", "date_to": "2026-04-02",
        })
        self.assertEqual(result.status_code, 200, result.text)
        self.assertGreater(result.json()["total_trades"], 0)
        detail = self.client.get(f'/api/backtest/{result.json()["id"]}').json()
        self.assertEqual(len(detail["trades"]), result.json()["total_trades"])

    def test_actual_cli_result_round_trips_through_history_and_detail(self):
        self.assertEqual(self.client.get("/api/backtest/runs").json(), [])
        response = self.client.post("/api/backtest/run", params={
            "strategy": "base_no_ev", "city": "NYC",
            "date_from": "2026-01-01", "date_to": "2026-01-01",
        })
        self.assertEqual(response.status_code, 200, response.text)
        run = response.json()
        self.assertEqual(run["status"], "done")
        self.assertEqual(run["total_trades"], 0)
        self.assertEqual(run["result_storage"], "files")
        history = self.client.get("/api/backtest/runs").json()
        self.assertEqual([row["id"] for row in history], [run["id"]])
        detail = self.client.get(f'/api/backtest/{run["id"]}').json()
        self.assertEqual(detail["meta"]["strategy"], "base_no_ev")
        self.assertEqual(detail["trades"], [])
        compare = self.client.get("/api/backtest/compare", params={"ids": run["id"]}).json()
        self.assertEqual(compare[0]["id"], run["id"])

    def test_registry_and_live_config_are_available_without_private_data(self):
        strategies = self.client.get("/api/backtest/strategies").json()
        self.assertIn("base_no_ev", [row["name"] for row in strategies])
        self.assertEqual(self.client.get("/api/live/positions").json(), [])
        self.assertEqual(self.client.get("/api/live/strategies").json()[0]["name"], "base_ev")

    def test_invalid_inputs_are_rejected_before_launching_runner(self):
        for params in ({"strategy": "missing"}, {"replay_mode": "single"},
                       {"interval_minutes": 5}, {"date_from": "invalid"},
                       {"date_from": "2026-02-02", "date_to": "2026-01-01"}):
            with self.subTest(params=params):
                self.assertEqual(self.client.post("/api/backtest/run", params=params).status_code, 400)
        self.assertEqual(self.client.get("/api/backtest/not-a-uuid").status_code, 404)


if __name__ == "__main__":
    unittest.main()
