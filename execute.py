"""
Execution bridge: reads execution_plan.json written by the Rust bot and
submits orders to Polymarket via py-clob-client.

Auth (all via environment variables — never hardcode):
    POLYMARKET_PRIVATE_KEY  — your Polygon wallet private key (hex, 0x-prefixed)
    POLYMARKET_FUNDER       — your wallet address (0x-prefixed)

Usage:
    python execute.py                         # read plan and execute (or dry-run if flagged)
    python execute.py --plan path/to/plan.json
    python execute.py --dry-run               # force dry-run regardless of plan flag
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import sys
import time
from datetime import datetime, timezone
from pathlib import Path


def _setup_logging(log_dir: Path | None) -> None:
    handlers: list[logging.Handler] = [logging.StreamHandler(sys.stdout)]
    if log_dir:
        log_dir.mkdir(parents=True, exist_ok=True)
        fh = logging.FileHandler(log_dir / "execute.log", encoding="utf-8")
        fh.setFormatter(logging.Formatter(
            "%(asctime)s %(levelname)s %(name)s — %(message)s",
            datefmt="%Y-%m-%d %H:%M:%S",
        ))
        handlers.append(fh)
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s — %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
        handlers=handlers,
    )

# Make sure the poly-weather package root is on sys.path when called from anywhere
sys.path.insert(0, str(Path(__file__).parent))

from data.db import Database

log = logging.getLogger(__name__)

DEFAULT_PLAN_PATH = Path(__file__).parent / "execution_plan.json"
DEFAULT_POSITIONS_PATH = Path(__file__).parent / "open_positions.json"

CLOB_HOST  = "https://clob.polymarket.com"
CHAIN_ID   = 137   # Polygon mainnet


# ── Helpers ───────────────────────────────────────────────────────────────────

def load_plan(path: Path) -> dict:
    if not path.exists():
        log.error("execution_plan.json not found at %s", path)
        sys.exit(1)
    with open(path) as f:
        return json.load(f)



def now_utc() -> str:
    return datetime.now(timezone.utc).isoformat()


# ── Order submission ──────────────────────────────────────────────────────────

def submit_trade(client, trade: dict) -> dict | None:
    """
    Submit a single NO buy order. Returns the order response dict or None on failure.
    """
    try:
        from py_clob_client.clob_types import OrderArgs, OrderType
        from py_clob_client.order_builder.constants import BUY

        order_args = OrderArgs(
            token_id=trade["no_token_id"],
            price=round(trade["effective_ask"], 2),  # CLOB prices are 2dp
            size=trade["shares"],
            side=BUY,
        )
        signed_order = client.create_order(order_args)
        response = client.post_order(signed_order, OrderType.GTC)
        log.info(
            "order submitted: bin=%s token=%s price=%.4f shares=%.1f response=%s",
            trade["bin_label"],
            trade["no_token_id"][:12] + "...",
            trade["effective_ask"],
            trade["shares"],
            response,
        )
        return response
    except Exception as e:
        log.error("order submission failed for %s: %s", trade["bin_label"], e)
        return None


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plan", default=str(DEFAULT_PLAN_PATH))
    parser.add_argument("--positions", default=str(DEFAULT_POSITIONS_PATH))
    parser.add_argument("--log-dir", default=None,
                        help="Directory for execute.log (passed by Rust bot)")
    parser.add_argument("--dry-run", action="store_true",
                        help="Log trades without submitting (overrides plan flag)")
    args = parser.parse_args()

    log_dir = Path(args.log_dir) if args.log_dir else None
    _setup_logging(log_dir)

    plan = load_plan(Path(args.plan))
    db = Database()

    dry_run = args.dry_run or plan.get("dry_run", True)

    log.info(
        "execution plan loaded: event=%s date=%s trades=%d total_stake=$%.2f dry_run=%s",
        plan.get("event_slug"),
        plan.get("target_date"),
        len(plan.get("trades", [])),
        plan.get("total_stake_usd", 0),
        dry_run,
    )

    if not plan.get("trades"):
        log.info("no trades in plan, exiting")
        return

    # ── Auth ─────────────────────────────────────────────────────────────────
    client = None
    if not dry_run:
        private_key = os.environ.get("POLYMARKET_PRIVATE_KEY")
        funder      = os.environ.get("POLYMARKET_FUNDER")

        if not private_key or not funder:
            log.error(
                "POLYMARKET_PRIVATE_KEY and POLYMARKET_FUNDER must be set for live trading"
            )
            sys.exit(1)

        try:
            from py_clob_client.client import ClobClient
            client = ClobClient(
                CLOB_HOST,
                key=private_key,
                chain_id=CHAIN_ID,
                funder=funder,
            )
            client.set_api_creds(client.create_or_derive_api_creds())
            log.info("Polymarket CLOB client authenticated (funder=%s)", funder[:10] + "...")
        except ImportError:
            log.error("py-clob-client is not installed. Run: pip install py-clob-client==0.34.6")
            sys.exit(1)
        except Exception as e:
            log.error("failed to authenticate CLOB client: %s", e)
            sys.exit(1)

    # ── Execute trades ────────────────────────────────────────────────────────
    for trade in plan["trades"]:
        log.info(
            "[%s] bin=%s p_model=%.4f ev_no=%.4f ask=%.4f shares=%.1f stake=$%.2f",
            "DRY-RUN" if dry_run else "LIVE",
            trade["bin_label"],
            trade["p_model"],
            trade["ev_no"],
            trade["effective_ask"],
            trade["shares"],
            trade["stake_usd"],
        )

        strategy_name = trade.get("strategy_name", "default")
        trade_rationale = json.dumps({
            "ev_no":         trade.get("ev_no"),
            "p_model":       trade.get("p_model"),
            "fee_rate":      trade.get("fee_rate"),
            "effective_ask": trade.get("effective_ask"),
        })

        if dry_run:
            log.info(
                "ORDER PLACED [PAPER] bin=%s ev_no=%.4f threshold=%.4f ask=%.4f shares=%.1f stake=$%.2f strategy=%s",
                trade["bin_label"], trade.get("ev_no", 0), 0.0,
                trade["effective_ask"], trade["shares"], trade["stake_usd"],
                strategy_name,
            )
            pos = {
                "token_id":       trade["no_token_id"],
                "market_id":      trade["market_id"],
                "bin_label":      trade["bin_label"],
                "event_slug":     plan["event_slug"],
                "target_date":    plan["target_date"],
                "entry_price":    trade["effective_ask"],
                "shares":         trade["shares"],
                "stake_usd":      trade["stake_usd"],
                "order_id":       None,
                "status":         "open",
                "pnl":            None,
                "opened_at":      now_utc(),
                "settled_at":     None,
                "is_paper":       True,
                "strategy_name":  strategy_name,
                "trade_rationale": trade_rationale,
            }
            db.insert_position(pos)
            continue

        # Live submission
        response = submit_trade(client, trade)
        if response:
            order_id = response.get("orderID") or response.get("id")
            log.info(
                "ORDER PLACED [LIVE] bin=%s ev_no=%.4f ask=%.4f shares=%.1f stake=$%.2f order_id=%s strategy=%s",
                trade["bin_label"], trade.get("ev_no", 0),
                trade["effective_ask"], trade["shares"], trade["stake_usd"],
                order_id, strategy_name,
            )
            pos = {
                "token_id":       trade["no_token_id"],
                "market_id":      trade["market_id"],
                "bin_label":      trade["bin_label"],
                "event_slug":     plan["event_slug"],
                "target_date":    plan["target_date"],
                "entry_price":    trade["effective_ask"],
                "shares":         trade["shares"],
                "stake_usd":      trade["stake_usd"],
                "order_id":       order_id,
                "status":         "open",
                "pnl":            None,
                "opened_at":      now_utc(),
                "settled_at":     None,
                "is_paper":       False,
                "strategy_name":  strategy_name,
                "trade_rationale": trade_rationale,
            }
            db.insert_position(pos)
        # Brief pause between orders to avoid rate limiting
        time.sleep(0.5)

    # ── Export open positions JSON for Rust bot ───────────────────────────────
    db.export_open_positions_json(Path(args.positions))
    open_count = len(db.get_open_positions())
    log.info("positions db updated: %d open positions", open_count)
    db.close()


if __name__ == "__main__":
    main()
