# Poly repository assessment

Reviewed on 2026-10-21 from the private `lynx-canadensis` repositories using
Frank's existing GitHub access. This is a source-code assessment, not a production
runtime or trading-performance audit.

## Conclusion

These projects fit a private `kangyining/poly` monorepo. Preserve the current
sibling folder names for the first import. The dashboard's path resolution and
the Python/Rust comparison harness already expect that arrangement. Combining
repositories simplifies coordinated changes, but it does not repair existing
data/API mismatches or make the Rust backend a complete replacement for Python.

## What each repository does

| Repository | Reviewed default branch | Implemented role |
| --- | --- | --- |
| [poly-weather](https://github.com/lynx-canadensis/poly-weather/tree/cf7cd42c680acd6ed1d30ae503f4dc76ba1954b3) | `main`, `cf7cd42` | Python recorder/backfills for weather and Polymarket observations; SQLite storage; forecast calibration; strategy plugins; read-only historical backtests and bankroll sweeps; settlement and an older order-execution bridge. |
| [poly-weather-rust](https://github.com/lynx-canadensis/poly-weather-rust/tree/9c3e60ea06451f0285053ee3b269c8f134408b0c) | `master`, `9c3e60e` | A migration of the Python backend: recorder/backfills, calibration, interval replay, bankroll/sweeps, status, paper execution and settlement. A Python/Rust comparison harness exists. Code explicitly rejects live order submission. It is distinct from the trading bot. |
| [poly-weather-bot](https://github.com/lynx-canadensis/poly-weather-bot/tree/175db66cc96d8a07a97ecebb0009d3e94627f16d) | `main`, `175db66` | Rust/Tokio event bus with weather polling, market discovery/websocket events, calibrated NO-side expected-value rules, portfolio limits, combined execution plans, native order submission and JSON portfolio/performance outputs. Checked-in config is paper mode. |
| [poly-dashboard](https://github.com/lynx-canadensis/poly-dashboard/tree/e507d0db5b8d8efb097fe0fc1e98e5b0b005bdb6) | `main`, `e507d0d` | React/Vite UI and FastAPI API for backtests, positions, PnL, settlements and observations. Imports Python city config and reads the Python SQLite database. |

## Integration issues found

1. **Dashboard backtest invocation is incompatible with the current backend.**
   `poly-dashboard/apps/api/main.py:125` always supplies `--replay-mode`; interval
   requests also supply `--interval-minutes`. The current parser in
   `poly-weather/analysis/phase1_backtest.py:1232` supports neither. The direct
   probe `python backtest.py --replay-mode single --days 1` exited 2 with
   `unrecognized arguments: --replay-mode single` before opening the database.

2. **The backtest response parser expects a different output format.**
   The API tries to JSON-parse stdout one line at a time, while the Python CLI
   ends with `json.dumps(payload, indent=2)`. It also expects historical runs and
   trades in SQLite tables, while current backtests open SQLite read-only and
   save CSV/Markdown artifacts. Removing the obsolete flags alone is insufficient.

3. **The strategy endpoint defaults to a missing legacy file.**
   The dashboard defaults to `poly-weather-bot/strategies.toml`, absent from the
   reviewed default branch. The bot uses `config.toml` with `[[strategies]]`;
   Python research uses `analysis/strategies/strategies.json`. The dashboard's
   existing TOML parser expects `strategy`, so changing only the filename will
   not reconcile the formats.

4. **The current bot and dashboard have different position stores.**
   The bot's `src/services/portfolio_service.rs` persists `paper_portfolio.json`
   and `open_positions.json`. The dashboard queries `poly-weather/db/data.db`.
   The bot's current execution service submits orders natively; it does not call
   Python `execute.py`. Older README architecture diagrams describe that older
   handoff. An adapter or common position-store contract is needed for current
   bot positions to appear consistently in the dashboard.

5. **Some runtime and generated artifacts are versioned.**
   Python tracks a 27,033,600-byte database plus a recorder log and position/plan
   JSON. The bot tracks portfolio/position JSON, performance CSV and a log. The
   Rust migration tracks a Python bytecode cache. Retaining exact files/history
   retains those artifacts. Keep research data deliberately and stop adding new
   runtime state to Git; do not imply an ignore rule removes history.

6. **Two Rust projects have different roles and build assumptions.**
   Keep both crates and their existing lockfiles initially. Do not delete Python
   based solely on the Rust migration's status document: that document records
   parity limits and excludes live execution. A Cargo workspace can be evaluated
   later; it would introduce a shared lockfile and build-output directory.

## Validation performed

- Cloned all four sources with their branches and histories.
- Inspected READMEs, manifests, configuration, CLI parsers, execution/portfolio
  services, dashboard API/frontend contracts and the Rust parity harness.
- Ran 32 Python tests across strategy math/plugins, weather contracts, mocked
  recorder/data paths and bankroll replay: all passed after placing test temp
  files inside the writable workspace. An initial run hit a sandbox temp-path
  permission error; the repeat resolved that environmental issue.
- Reproduced the dashboard's unsupported backtest CLI argument.
- Reviewed the source Rust verification notes as historical claims; did not
  rerun Rust builds, parity benchmarks, frontend builds or live network flows.
- Did not start a recorder, trading loop or order-submission command.

## Recommended sequence

1. Import the four default trees into sibling directories without changing
   application code. Keep original commits and preserve every source branch
   under `archive/<repo>/<branch>`. Verify tree and commit identity.
2. Publish the combined repository privately under `kangyining` if requested.
   The source repositories can continue serving existing deployments.
3. Repair the dashboard/backtest contract in a focused change: use supported
   parameters, parse the full JSON payload, read the strategy registry, and
   choose an artifact/run-index contract compatible with read-only backtests.
4. Add an explicit adapter for the bot's current JSON position/portfolio state,
   or deliberately migrate both sides to a common store with schema tests.
5. Separate runtime state from code while preserving necessary research data.
   Add targeted Python/Rust/frontend checks after establishing their baselines.
6. Consider shared Rust modules or retiring Python only after the existing parity
   harness and relevant deployment workflows validate that decision.

GitHub documents [subtree merges](https://docs.github.com/en/get-started/using-git/about-git-subtree-merges)
as a way to bring multiple projects into one repository. Cargo's
[workspace reference](https://doc.rust-lang.org/cargo/reference/workspaces.html)
explains the shared lockfile/build behavior that makes a Rust workspace a
separate dependency-management decision from the repository import.
