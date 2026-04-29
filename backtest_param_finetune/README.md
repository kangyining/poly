# Backtest Parameter Finetune

Grid-searches the strategy lab plugins and their tunable parameters. Each
combination is run through
`analysis.phase1_backtest.run_backtest`. SQLite is used as the historical data
source only; results are written to CSV and Markdown reports under
`backtest_param_finetune/results/`.

The sweep always uses 1-minute interval replay. Default sizing sweeps
`desired_shares=1..10`, does not use `funding_pct`, and has no daily/global
stake cap unless you pass `--daily-max-stake-usd`.

## Quick Start: NYC All Available Data

From `poly-weather`:

```bash
source .venv/bin/activate

python backtest_param_finetune/param_sweep.py \
  --backtest-ready-strategies \
  --city NYC \
  --date-from 2026-04-01 \
  --date-to 2026-04-25 \
  --sweep-name nyc_april_all_strategies_fixed10_v1 \
  --jobs 4 \
  --rank-by roi \
  --min-trades 5
```

This automatically:

- sweeps all currently executable strategy plugins
- uses `NYC`
- tests the built-in quick grid plus strategy-specific parameter grids
- sweeps `desired_shares` from `1` through `10`
- leaves stake uncapped
- runs up to 4 parameter combinations in parallel
- keeps results out of SQLite
- writes global, per-strategy, and best-by-strategy CSV/Markdown artifacts under `backtest_param_finetune/results/`

## Custom Date Range

```bash
python backtest_param_finetune/param_sweep.py \
  --strategy dynamic_sigma_ev \
  --strategy maker_quote_sim \
  --sweep-name nyc_april_dynamic_maker_fixed10_v1 \
  --city NYC \
  --date-from 2026-04-01 \
  --date-to 2026-04-25 \
  --min-ev-thresholds 0.01:0.10:0.01 \
  --desired-shares 1:10:1 \
  --strategy-param-grid dynamic_sigma_ev:dynamic_sigma_min=0.4:1.2:0.2 \
  --strategy-param-grid maker_quote_sim:tick_size=0.005,0.01,0.02 \
  --jobs 4 \
  --rank-by roi \
  --min-trades 5
```

## Parameters

- `min_ev_threshold`: minimum fee-adjusted NO edge per share required to trade.
- `desired_shares`: target shares per selected candidate. Defaults to `1:10:1`.
- `fixed_trade_stake_usd`: optional fixed stake per selected candidate. Omit it when sweeping `desired_shares`.
- `daily_max_stake_usd`: optional per-day stake cap. Omit it for uncapped research sweeps.
- `max_stake_usd`: deprecated alias for the daily cap.
- `fee_rate`: fee rate used by backtest when no snapshot fee is available.
- `--strategy`, `--strategy-group`, `--all-strategies`: choose what to sweep.
- `--backtest-ready-strategies`: sweep only executable plugins.
- `--param-grid key=v1,v2`: add or override a parameter grid for every selected strategy.
- `--strategy-param-grid strategy:key=v1,v2`: add or override a parameter grid for one strategy.
- `--jobs N`: run parameter combinations in parallel. Start with `4`, then increase if CPU and disk stay comfortable.

Range syntax is inclusive:

```text
--desired-shares 1:10:1
--min-ev-thresholds 0.01:0.10:0.01
```

Ranking by `roi` uses:

```text
roi = total_pnl / total_stake
```

Rows below `--min-trades` are pushed to the bottom of the ranking so tiny
sample-size winners do not look better than they are.

## Output

The script prints JSON with the top rows and always writes:

```text
backtest_param_finetune/results/<sweep-name>.csv
backtest_param_finetune/results/<sweep-name>.md
backtest_param_finetune/results/<sweep-name>__<strategy>.csv
backtest_param_finetune/results/<sweep-name>__<strategy>.md
backtest_param_finetune/results/<sweep-name>__best_by_strategy.md
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
