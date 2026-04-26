# Backtest Parameter Finetune

Grid-searches the current calibrated NO-EV backtest parameters without changing
the strategy algorithm. Each combination is run through
`analysis.phase1_backtest.run_backtest`, so results are persisted to the normal
SQLite `backtests` and `positions` tables. A ranked CSV summary is also written
under `backtest_param_finetune/results/`.

## Quick Start: NYC All Available Data

From `poly-weather`:

```bash
source .venv/bin/activate

python backtest_param_finetune/param_sweep.py \
  --nyc-all-available \
  --sweep-name nyc_all_available_full_v1 \
  --replay-mode single \
  --rank-by roi \
  --min-trades 10
```

This automatically:

- uses `NYC`
- finds all currently backtest-ready NYC dates in `db/data.db`
- tests the full built-in grid for every supported parameter
- persists every combo as a normal backtest run
- writes both CSV and Markdown report files under `backtest_param_finetune/results/`

## Custom Date Range

```bash
python backtest_param_finetune/param_sweep.py \
  --sweep-name nyc_april_single_v1 \
  --city NYC \
  --date-from 2026-04-01 \
  --date-to 2026-04-19 \
  --replay-mode single \
  --min-ev-thresholds 0.02,0.03,0.04,0.05,0.06 \
  --desired-shares 2.5,5,10 \
  --funding-pcts 0.5,1.0 \
  --max-stake-usd 25,50,100 \
  --rank-by roi \
  --min-trades 5
```

For interval replay:

```bash
python backtest_param_finetune/param_sweep.py \
  --sweep-name nyc_april_interval_5m_v1 \
  --city NYC \
  --date-from 2026-04-01 \
  --date-to 2026-04-19 \
  --replay-mode interval \
  --interval-minutes 5 \
  --min-ev-thresholds 0.02,0.03,0.04,0.05 \
  --desired-shares 5,10 \
  --funding-pcts 1.0 \
  --max-stake-usd 25,50 \
  --rank-by roi \
  --min-trades 10
```

## Parameters

- `min_ev_threshold`: minimum fee-adjusted NO edge per share required to trade.
- `desired_shares`: target shares before applying `funding_pct`.
- `funding_pct`: multiplier on `desired_shares`.
- `max_stake_usd`: per-day stake cap inside the current backtest engine.
- `fee_rate`: fee rate used by backtest when no snapshot fee is available.

Effective shares are:

```text
shares = desired_shares * funding_pct
```

Ranking by `roi` uses:

```text
roi = total_pnl / total_stake
```

Rows below `--min-trades` are pushed to the bottom of the ranking so tiny
sample-size winners do not look better than they are.

## Output

The script prints JSON with the top rows and writes:

```text
backtest_param_finetune/results/<sweep-name>.csv
```

When `--report` or `--nyc-all-available` is used, it also writes:

```text
backtest_param_finetune/results/<sweep-name>.md
```

Each row includes the generated `backtest_id`, tested params, PnL, stake, ROI,
Sharpe, win rate, and the full params JSON stored with the run.

## Caution

Use a train/validation split. For example:

```text
tune:      2026-02-05 through 2026-03-31
validate:  2026-04-01 through 2026-04-19
```

Do not pick parameters solely from the same period you later report as
performance.
