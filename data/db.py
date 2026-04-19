"""
SQLite storage layer.
"""

import json
import sqlite3
import logging
from datetime import datetime
from pathlib import Path

from data.models import MarketSnapshot, METARRecord, ForecastRecord, SettlementRecord

log = logging.getLogger(__name__)

SCHEMA_PATH = Path(__file__).parent.parent / "db" / "schema.sql"
DEFAULT_DB   = Path(__file__).parent.parent / "db" / "data.db"


class Database:
    def __init__(self, db_path: Path = DEFAULT_DB):
        db_path.parent.mkdir(parents=True, exist_ok=True)
        self.conn = sqlite3.connect(str(db_path), check_same_thread=False)
        self.conn.row_factory = sqlite3.Row
        self._apply_schema()
        log.info("Database ready: %s", db_path)

    def _apply_schema(self):
        sql = SCHEMA_PATH.read_text()
        self.conn.executescript(sql)
        self.conn.commit()

    # ── market snapshots ──────────────────────────────────────────────────

    def insert_snapshot(self, s: MarketSnapshot):
        self.conn.execute("""
            INSERT INTO market_snapshots (
                timestamp, city, target_date, event_id, event_slug,
                bin_label, bin_index, token_id_yes, token_id_no,
                yes_price, no_price,
                yes_best_bid, yes_best_ask, no_best_bid, no_best_ask,
                book_depth_bid, book_depth_ask, volume_24h, liquidity
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """, (
            s.timestamp.isoformat(), s.city, s.target_date,
            s.event_id, s.event_slug, s.bin_label, s.bin_index,
            s.token_id_yes, s.token_id_no,
            s.yes_price, s.no_price,
            s.yes_best_bid, s.yes_best_ask, s.no_best_bid, s.no_best_ask,
            s.book_depth_bid, s.book_depth_ask, s.volume_24h, s.liquidity,
        ))
        self.conn.commit()

    def snapshot_count(self) -> int:
        return self.conn.execute(
            "SELECT COUNT(*) FROM market_snapshots"
        ).fetchone()[0]

    # ── METAR records ─────────────────────────────────────────────────────

    def insert_metar(self, m: METARRecord):
        self.conn.execute("""
            INSERT INTO metar_records (
                timestamp, icao, city, temp_c, temp_f,
                dewpoint_c, obs_time, raw_metar
            ) VALUES (?,?,?,?,?,?,?,?)
        """, (
            m.timestamp.isoformat(), m.icao, m.city,
            m.temp_c, m.temp_f, m.dewpoint_c,
            m.obs_time.isoformat() if m.obs_time else None,
            m.raw_metar,
        ))
        self.conn.commit()

    # ── forecast records ──────────────────────────────────────────────────

    def insert_forecast(self, f: ForecastRecord):
        self.conn.execute("""
            INSERT INTO forecast_records (
                timestamp, city, target_date, source,
                temp_max_ecmwf_c, temp_max_gfs_c,
                temp_max_ecmwf_f, temp_max_gfs_f,
                model_spread_c, forecast_lead_hours, raw_json
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?)
        """, (
            f.timestamp.isoformat(), f.city, f.target_date, f.source,
            f.temp_max_ecmwf_c, f.temp_max_gfs_c,
            f.temp_max_ecmwf_f, f.temp_max_gfs_f,
            f.model_spread_c, f.forecast_lead_hours, f.raw_json,
        ))
        self.conn.commit()

    # ── settlement records ────────────────────────────────────────────────

    def insert_settlement(self, s: SettlementRecord):
        self.conn.execute("""
            INSERT OR IGNORE INTO settlement_records (
                city, target_date, event_id, event_slug,
                winning_bin, settlement_temp_f,
                wunderground_station, resolved_at, raw_outcome_prices
            ) VALUES (?,?,?,?,?,?,?,?,?)
        """, (
            s.city, s.target_date, s.event_id, s.event_slug,
            s.winning_bin, s.settlement_temp_f,
            s.wunderground_station,
            s.resolved_at.isoformat() if s.resolved_at else None,
            s.raw_outcome_prices,
        ))
        self.conn.commit()

    def settlement_count(self) -> int:
        return self.conn.execute(
            "SELECT COUNT(*) FROM settlement_records"
        ).fetchone()[0]

    def settlement_exists(self, city: str, target_date: str) -> bool:
        row = self.conn.execute(
            "SELECT 1 FROM settlement_records WHERE city=? AND target_date=?",
            (city, target_date)
        ).fetchone()
        return row is not None

    # ── price history ────────────────────────────────────────────────────

    def insert_price_history(self, city: str, target_date: str, bin_label: str,
                              bin_index: int, token_id_yes: str, timestamp: str,
                              yes_price: float, hours_to_close: float,
                              source: str = 'clob-backfill'):
        self.conn.execute("""
            INSERT OR IGNORE INTO price_history (
                city, target_date, bin_label, bin_index, token_id_yes,
                timestamp, yes_price, hours_to_close, source
            ) VALUES (?,?,?,?,?,?,?,?,?)
        """, (city, target_date, bin_label, bin_index, token_id_yes,
              timestamp, yes_price, hours_to_close, source))
        self.conn.commit()

    def price_history_exists(self, city: str, target_date: str) -> bool:
        row = self.conn.execute(
            "SELECT 1 FROM price_history WHERE city=? AND target_date=? LIMIT 1",
            (city, target_date)
        ).fetchone()
        return row is not None

    # ── TAF records ───────────────────────────────────────────────────────

    def insert_taf(self, icao: str, city: str, issue_time,
                   valid_from, valid_to, raw_taf: str):
        from datetime import datetime, timezone
        now = datetime.now(timezone.utc).isoformat()
        self.conn.execute("""
            INSERT INTO taf_records (
                timestamp, icao, city, issue_time, valid_from, valid_to, raw_taf
            ) VALUES (?,?,?,?,?,?,?)
        """, (
            now, icao, city,
            issue_time.isoformat() if hasattr(issue_time, 'isoformat') else issue_time,
            valid_from.isoformat() if hasattr(valid_from, 'isoformat') else valid_from,
            valid_to.isoformat() if hasattr(valid_to, 'isoformat') else valid_to,
            raw_taf,
        ))
        self.conn.commit()

    # ── NWS forecast records ──────────────────────────────────────────────

    def insert_nws_forecast(self, city: str, target_date: str, period_name: str,
                             is_daytime: bool, temp_f: float,
                             short_forecast: str, lead_hours: float):
        from datetime import datetime, timezone
        now = datetime.now(timezone.utc).isoformat()
        self.conn.execute("""
            INSERT INTO nws_forecasts (
                timestamp, city, target_date, period_name,
                is_daytime, temp_f, short_forecast, forecast_lead_hours
            ) VALUES (?,?,?,?,?,?,?,?)
        """, (
            now, city, target_date, period_name,
            1 if is_daytime else 0, temp_f, short_forecast, lead_hours,
        ))
        self.conn.commit()

    # ── positions ─────────────────────────────────────────────────────────

    def insert_position(self, pos: dict) -> int:
        """
        Insert a position from open_positions.json dict (written by execute.py).
        Returns the new row id. Ignores duplicates (same token_id + order_id).
        """
        cur = self.conn.execute("""
            INSERT OR IGNORE INTO positions (
                token_id, market_id, bin_label, event_slug, target_date,
                entry_price, shares, stake_usd, order_id,
                status, pnl, opened_at, settled_at, is_paper,
                strategy_name, backtest_id, trade_rationale
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """, (
            pos["token_id"], pos.get("market_id", ""),
            pos["bin_label"], pos["event_slug"], pos["target_date"],
            pos["entry_price"], pos["shares"], pos["stake_usd"],
            pos.get("order_id"),
            pos.get("status", "open"), pos.get("pnl"),
            pos["opened_at"], pos.get("settled_at"),
            1 if pos.get("is_paper", True) else 0,
            pos.get("strategy_name", "default"),
            pos.get("backtest_id"),
            pos.get("trade_rationale"),
        ))
        self.conn.commit()
        return cur.lastrowid

    def get_open_positions(self) -> list[dict]:
        """Return all open positions as plain dicts."""
        rows = self.conn.execute(
            "SELECT * FROM positions WHERE status='open' ORDER BY opened_at"
        ).fetchall()
        return [dict(r) for r in rows]

    def close_position(self, position_id: int, pnl: float, settled_at: str) -> None:
        """Mark a position closed with its PnL."""
        self.conn.execute(
            "UPDATE positions SET status='closed', pnl=?, settled_at=? WHERE id=?",
            (pnl, settled_at, position_id),
        )
        self.conn.commit()

    def export_open_positions_json(self, path) -> None:
        """
        Write open_positions.json consumed by the Rust bot to skip already-held bins.
        Only exports token_id and status so the file stays small.
        """
        import json
        rows = self.get_open_positions()
        out = [
            {
                "token_id":   r["token_id"],
                "market_id":  r["market_id"],
                "bin_label":  r["bin_label"],
                "event_slug": r["event_slug"],
                "target_date": r["target_date"],
                "entry_price": r["entry_price"],
                "shares":     r["shares"],
                "stake_usd":  r["stake_usd"],
                "order_id":   r["order_id"],
                "status":     r["status"],
                "pnl":        r["pnl"],
                "opened_at":  r["opened_at"],
                "settled_at": r["settled_at"],
                "is_paper":   bool(r["is_paper"]),
            }
            for r in rows
        ]
        from pathlib import Path
        Path(path).write_text(json.dumps(out, indent=2))
        log.info("exported %d open positions -> %s", len(out), path)

    def close(self):
        self.conn.close()
