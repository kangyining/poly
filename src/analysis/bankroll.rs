use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::backtest::BacktestTradeRow;
use crate::db::parse_ts;

#[derive(Debug, Clone)]
pub struct ReplayCandidate {
    pub strategy_name: String,
    pub strategy_plugin: String,
    pub city: String,
    pub target_date: String,
    pub event_slug: String,
    pub bin_label: String,
    pub bin_index: i64,
    pub side: String,
    pub opened_at: DateTime<Utc>,
    pub resolved_at: DateTime<Utc>,
    pub desired_stake_usd: f64,
    pub desired_shares: f64,
    pub pnl: f64,
    pub won: bool,
    pub effective_ask: f64,
    pub fee_rate: f64,
    pub p_model: Option<f64>,
    pub edge: Option<f64>,
    pub fill_source: String,
    pub token_id: String,
    pub market_id: String,
    pub metadata: Value,
}

impl ReplayCandidate {
    pub fn from_trade_row(
        row: &BacktestTradeRow,
        city: Option<&str>,
        strategy_plugin: &str,
    ) -> Self {
        let metadata = serde_json::from_str::<Value>(&row.trade_rationale).unwrap_or(Value::Null);
        let side = metadata
            .get("side")
            .and_then(Value::as_str)
            .unwrap_or("NO")
            .to_string();
        let fill_source = metadata
            .get("fill_source")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let p_model = metadata.get("p_model").and_then(Value::as_f64);
        let edge = metadata
            .get("edge")
            .or_else(|| metadata.get("ev_no"))
            .and_then(Value::as_f64);
        Self {
            strategy_name: row.strategy_name.clone(),
            strategy_plugin: strategy_plugin.to_string(),
            city: city.unwrap_or("").to_string(),
            target_date: row.target_date.clone(),
            event_slug: row.event_slug.clone(),
            bin_label: row.bin_label.clone(),
            bin_index: metadata
                .get("bin_index")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            side,
            opened_at: parse_ts(&row.opened_at).unwrap_or_else(Utc::now),
            resolved_at: parse_ts(&row.settled_at).unwrap_or_else(Utc::now),
            desired_stake_usd: row.stake_usd,
            desired_shares: row.shares,
            pnl: row.pnl,
            won: row.pnl > 0.0,
            effective_ask: row.entry_price,
            fee_rate: row.fee_rate,
            p_model,
            edge,
            fill_source,
            token_id: row.token_id.clone(),
            market_id: row.market_id.clone(),
            metadata,
        }
    }
}

#[derive(Debug, Clone)]
struct OpenPosition {
    event: ReplayCandidate,
    allocated_stake_usd: f64,
    allocated_shares: f64,
    pnl: f64,
    scale_factor: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerTrade {
    pub strategy_name: String,
    pub strategy_plugin: String,
    pub city: String,
    pub target_date: String,
    pub event_slug: String,
    pub bin_label: String,
    pub bin_index: i64,
    pub side: String,
    pub entry_price: f64,
    pub fee_rate: f64,
    pub desired_stake_usd: f64,
    pub allocated_stake_usd: f64,
    pub stake_usd: f64,
    pub desired_shares: f64,
    pub shares: f64,
    pub scale_factor: f64,
    pub pnl: f64,
    pub won: bool,
    pub cash_before: f64,
    pub cash_after: f64,
    pub equity_after: f64,
    pub opened_at: String,
    pub resolved_at: String,
    pub settled_at: String,
    pub token_id: String,
    pub market_id: String,
    pub fill_source: String,
    pub p_model: Option<f64>,
    pub edge: Option<f64>,
    pub metadata: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerSkip {
    pub timestamp: String,
    pub city: String,
    pub target_date: String,
    pub event_slug: String,
    pub bin_label: String,
    pub side: String,
    pub desired_stake_usd: f64,
    pub available_cash: f64,
    pub min_trade_stake_usd: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquityPoint {
    pub timestamp: String,
    pub event_type: String,
    pub city: String,
    pub target_date: String,
    pub cash: f64,
    pub locked_capital: f64,
    pub equity: f64,
    pub drawdown: f64,
    pub capital_utilization: f64,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerSummary {
    pub initial_bankroll: f64,
    pub final_equity: f64,
    pub ending_cash: f64,
    pub portfolio_return: Option<f64>,
    pub realized_pnl: f64,
    pub total_pnl: f64,
    pub open_locked_capital: f64,
    pub max_drawdown: f64,
    pub max_locked_capital: f64,
    pub max_capital_utilization: f64,
    pub avg_capital_utilization: f64,
    pub trades_opened: usize,
    pub trades_scaled: usize,
    pub trades_skipped_insufficient_cash: usize,
    pub accepted_trades: usize,
    pub skipped_trades: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub total_stake: f64,
    pub turnover_yield: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerResult {
    pub summary: LedgerSummary,
    pub trades: Vec<LedgerTrade>,
    pub skips: Vec<LedgerSkip>,
    pub equity_curve: Vec<EquityPoint>,
}

pub fn apply_bankroll_ledger(
    mut candidates: Vec<ReplayCandidate>,
    initial_bankroll: f64,
    min_trade_stake_usd: f64,
    allow_scale_down: bool,
) -> LedgerResult {
    candidates.sort_by_key(|p| {
        (
            p.opened_at,
            p.city.clone(),
            p.target_date.clone(),
            p.bin_index,
        )
    });
    let mut cash = initial_bankroll;
    let mut open_positions: Vec<OpenPosition> = Vec::new();
    let mut trades = Vec::new();
    let mut skips = Vec::new();
    let mut equity_curve = Vec::new();
    let mut realized_pnl = 0.0;
    let mut total_stake = 0.0;
    let mut trades_scaled = 0;
    let mut trades_skipped_insufficient_cash = 0;
    let mut peak_equity = initial_bankroll;
    let mut max_drawdown = 0.0;
    let mut max_locked_capital = 0.0;
    let mut max_capital_utilization = 0.0;
    let mut utilization_seconds = 0.0;
    let mut elapsed_seconds = 0.0;
    let mut last_ts: Option<DateTime<Utc>> = None;
    let mut last_utilization = 0.0;

    for event in candidates {
        settle_due(
            event.opened_at,
            &mut cash,
            &mut realized_pnl,
            &mut open_positions,
            &mut equity_curve,
            &mut peak_equity,
            &mut max_drawdown,
            &mut max_locked_capital,
            &mut max_capital_utilization,
            &mut utilization_seconds,
            &mut elapsed_seconds,
            &mut last_ts,
            &mut last_utilization,
        );

        let cash_before = cash;
        let desired_stake = event.desired_stake_usd;
        if desired_stake <= 0.0 {
            continue;
        }

        let (allocated_stake, scale_factor) = if cash >= desired_stake {
            (desired_stake, 1.0)
        } else if allow_scale_down && cash >= min_trade_stake_usd {
            trades_scaled += 1;
            (cash, cash / desired_stake)
        } else {
            trades_skipped_insufficient_cash += 1;
            skips.push(LedgerSkip {
                timestamp: event.opened_at.to_rfc3339(),
                city: event.city.clone(),
                target_date: event.target_date.clone(),
                event_slug: event.event_slug.clone(),
                bin_label: event.bin_label.clone(),
                side: event.side.clone(),
                desired_stake_usd: round6(desired_stake),
                available_cash: round6(cash),
                min_trade_stake_usd: round6(min_trade_stake_usd),
                reason: "insufficient_cash".to_string(),
            });
            mark(
                event.opened_at,
                "skip",
                &event.city,
                &event.target_date,
                &event.bin_label,
                cash,
                &open_positions,
                &mut equity_curve,
                &mut peak_equity,
                &mut max_drawdown,
                &mut max_locked_capital,
                &mut max_capital_utilization,
                &mut utilization_seconds,
                &mut elapsed_seconds,
                &mut last_ts,
                &mut last_utilization,
            );
            continue;
        };

        let allocated_shares = event.desired_shares * scale_factor;
        let scaled_pnl = event.pnl * scale_factor;
        cash -= allocated_stake;
        open_positions.push(OpenPosition {
            event: event.clone(),
            allocated_stake_usd: allocated_stake,
            allocated_shares,
            pnl: scaled_pnl,
            scale_factor,
        });
        total_stake += allocated_stake;
        mark(
            event.opened_at,
            "open",
            &event.city,
            &event.target_date,
            &event.bin_label,
            cash,
            &open_positions,
            &mut equity_curve,
            &mut peak_equity,
            &mut max_drawdown,
            &mut max_locked_capital,
            &mut max_capital_utilization,
            &mut utilization_seconds,
            &mut elapsed_seconds,
            &mut last_ts,
            &mut last_utilization,
        );
        trades.push(LedgerTrade {
            strategy_name: event.strategy_name.clone(),
            strategy_plugin: event.strategy_plugin.clone(),
            city: event.city.clone(),
            target_date: event.target_date.clone(),
            event_slug: event.event_slug.clone(),
            bin_label: event.bin_label.clone(),
            bin_index: event.bin_index,
            side: event.side.clone(),
            entry_price: event.effective_ask,
            fee_rate: event.fee_rate,
            desired_stake_usd: round6(desired_stake),
            allocated_stake_usd: round6(allocated_stake),
            stake_usd: round6(allocated_stake),
            desired_shares: round8(event.desired_shares),
            shares: round8(allocated_shares),
            scale_factor: round8(scale_factor),
            pnl: round6(scaled_pnl),
            won: event.won,
            cash_before: round6(cash_before),
            cash_after: round6(cash),
            equity_after: round6(equity(cash, &open_positions)),
            opened_at: event.opened_at.to_rfc3339(),
            resolved_at: event.resolved_at.to_rfc3339(),
            settled_at: event.resolved_at.to_rfc3339(),
            token_id: event.token_id.clone(),
            market_id: event.market_id.clone(),
            fill_source: event.fill_source.clone(),
            p_model: event.p_model,
            edge: event.edge,
            metadata: event.metadata.to_string(),
        });
    }

    while let Some(next) = open_positions.iter().map(|pos| pos.event.resolved_at).min() {
        settle_due(
            next,
            &mut cash,
            &mut realized_pnl,
            &mut open_positions,
            &mut equity_curve,
            &mut peak_equity,
            &mut max_drawdown,
            &mut max_locked_capital,
            &mut max_capital_utilization,
            &mut utilization_seconds,
            &mut elapsed_seconds,
            &mut last_ts,
            &mut last_utilization,
        );
    }

    let final_locked = locked(&open_positions);
    let final_equity = cash + final_locked;
    let avg_utilization = if elapsed_seconds > 0.0 {
        utilization_seconds / elapsed_seconds
    } else {
        0.0
    };
    let wins = trades.iter().filter(|trade| trade.won).count();
    LedgerResult {
        summary: LedgerSummary {
            initial_bankroll: round4(initial_bankroll),
            final_equity: round4(final_equity),
            ending_cash: round4(cash),
            portfolio_return: (initial_bankroll != 0.0)
                .then(|| round6((final_equity - initial_bankroll) / initial_bankroll)),
            realized_pnl: round4(realized_pnl),
            total_pnl: round4(realized_pnl),
            open_locked_capital: round4(final_locked),
            max_drawdown: round6(max_drawdown),
            max_locked_capital: round4(max_locked_capital),
            max_capital_utilization: round6(max_capital_utilization),
            avg_capital_utilization: round6(avg_utilization),
            trades_opened: trades.len(),
            trades_scaled,
            trades_skipped_insufficient_cash,
            accepted_trades: trades.len(),
            skipped_trades: skips.len(),
            wins,
            win_rate: if trades.is_empty() {
                0.0
            } else {
                round4(wins as f64 / trades.len() as f64)
            },
            total_stake: round4(total_stake),
            turnover_yield: (total_stake > 0.0).then(|| round6(realized_pnl / total_stake)),
        },
        trades,
        skips,
        equity_curve,
    }
}

fn settle_due(
    ts: DateTime<Utc>,
    cash: &mut f64,
    realized_pnl: &mut f64,
    open_positions: &mut Vec<OpenPosition>,
    equity_curve: &mut Vec<EquityPoint>,
    peak_equity: &mut f64,
    max_drawdown: &mut f64,
    max_locked_capital: &mut f64,
    max_capital_utilization: &mut f64,
    utilization_seconds: &mut f64,
    elapsed_seconds: &mut f64,
    last_ts: &mut Option<DateTime<Utc>>,
    last_utilization: &mut f64,
) {
    open_positions.sort_by_key(|pos| {
        (
            pos.event.resolved_at,
            pos.event.city.clone(),
            pos.event.target_date.clone(),
            pos.event.bin_index,
        )
    });
    let mut idx = 0;
    while idx < open_positions.len() {
        if open_positions[idx].event.resolved_at > ts {
            idx += 1;
            continue;
        }
        let pos = open_positions.remove(idx);
        *cash += pos.allocated_stake_usd + pos.pnl;
        *realized_pnl += pos.pnl;
        let note = format!(
            "{} shares; scale {:.8}",
            round8(pos.allocated_shares),
            round8(pos.scale_factor)
        );
        mark_with_note(
            pos.event.resolved_at,
            "settle",
            &pos.event.city,
            &pos.event.target_date,
            &pos.event.bin_label,
            &note,
            *cash,
            open_positions,
            equity_curve,
            peak_equity,
            max_drawdown,
            max_locked_capital,
            max_capital_utilization,
            utilization_seconds,
            elapsed_seconds,
            last_ts,
            last_utilization,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn mark(
    ts: DateTime<Utc>,
    event_type: &str,
    city: &str,
    target_date: &str,
    note: &str,
    cash: f64,
    open_positions: &[OpenPosition],
    equity_curve: &mut Vec<EquityPoint>,
    peak_equity: &mut f64,
    max_drawdown: &mut f64,
    max_locked_capital: &mut f64,
    max_capital_utilization: &mut f64,
    utilization_seconds: &mut f64,
    elapsed_seconds: &mut f64,
    last_ts: &mut Option<DateTime<Utc>>,
    last_utilization: &mut f64,
) {
    mark_with_note(
        ts,
        event_type,
        city,
        target_date,
        "",
        note,
        cash,
        open_positions,
        equity_curve,
        peak_equity,
        max_drawdown,
        max_locked_capital,
        max_capital_utilization,
        utilization_seconds,
        elapsed_seconds,
        last_ts,
        last_utilization,
    );
}

#[allow(clippy::too_many_arguments)]
fn mark_with_note(
    ts: DateTime<Utc>,
    event_type: &str,
    city: &str,
    target_date: &str,
    bin_label: &str,
    note: &str,
    cash: f64,
    open_positions: &[OpenPosition],
    equity_curve: &mut Vec<EquityPoint>,
    peak_equity: &mut f64,
    max_drawdown: &mut f64,
    max_locked_capital: &mut f64,
    max_capital_utilization: &mut f64,
    utilization_seconds: &mut f64,
    elapsed_seconds: &mut f64,
    last_ts: &mut Option<DateTime<Utc>>,
    last_utilization: &mut f64,
) {
    if let Some(prev) = last_ts {
        if ts > *prev {
            let delta = (ts - *prev).num_milliseconds() as f64 / 1000.0;
            *utilization_seconds += *last_utilization * delta;
            *elapsed_seconds += delta;
        }
    }

    let locked_capital = locked(open_positions);
    let eq = cash + locked_capital;
    *peak_equity = peak_equity.max(eq);
    let drawdown = if *peak_equity > 0.0 {
        (*peak_equity - eq) / *peak_equity
    } else {
        0.0
    };
    let utilization = if eq > 0.0 { locked_capital / eq } else { 0.0 };
    *max_drawdown = max_drawdown.max(drawdown);
    *max_locked_capital = max_locked_capital.max(locked_capital);
    *max_capital_utilization = max_capital_utilization.max(utilization);
    equity_curve.push(EquityPoint {
        timestamp: ts.to_rfc3339(),
        event_type: event_type.to_string(),
        city: city.to_string(),
        target_date: target_date.to_string(),
        cash: round6(cash),
        locked_capital: round6(locked_capital),
        equity: round6(eq),
        drawdown: round8(drawdown),
        capital_utilization: round8(utilization),
        note: if note.is_empty() {
            bin_label.to_string()
        } else {
            note.to_string()
        },
    });
    *last_ts = Some(ts);
    *last_utilization = utilization;
}

fn equity(cash: f64, open_positions: &[OpenPosition]) -> f64 {
    cash + locked(open_positions)
}

fn locked(open_positions: &[OpenPosition]) -> f64 {
    open_positions
        .iter()
        .map(|pos| pos.allocated_stake_usd)
        .sum()
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

fn round8(v: f64) -> f64 {
    (v * 100_000_000.0).round() / 100_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn candidate(
        opened_at: DateTime<Utc>,
        resolved_at: DateTime<Utc>,
        stake: f64,
        pnl: f64,
    ) -> ReplayCandidate {
        ReplayCandidate {
            strategy_name: "base_ev".into(),
            strategy_plugin: "base_no_ev".into(),
            city: "NYC".into(),
            target_date: opened_at.date_naive().to_string(),
            event_slug: "highest-temperature-in-nyc-on-2026-04-01".into(),
            bin_label: "70-71°F".into(),
            bin_index: 0,
            side: "NO".into(),
            opened_at,
            resolved_at,
            desired_stake_usd: stake,
            desired_shares: 10.0,
            pnl,
            won: pnl > 0.0,
            effective_ask: 0.5,
            fee_rate: 0.05,
            p_model: Some(0.1),
            edge: Some(0.1),
            fill_source: "price_history_proxy".into(),
            token_id: "token".into(),
            market_id: "market".into(),
            metadata: Value::Null,
        }
    }

    #[test]
    fn full_size_trade_reserves_and_releases_cash() {
        let opened = "2026-04-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let result = apply_bankroll_ledger(
            vec![candidate(opened, opened + Duration::days(1), 10.0, 3.0)],
            100.0,
            5.0,
            true,
        );
        assert_eq!(result.summary.trades_opened, 1);
        assert_eq!(result.summary.realized_pnl, 3.0);
        assert_eq!(result.summary.final_equity, 103.0);
        assert_eq!(result.trades[0].cash_before, 100.0);
        assert_eq!(result.trades[0].cash_after, 90.0);
    }

    #[test]
    fn insufficient_cash_below_minimum_skips_candidate() {
        let opened = "2026-04-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let result = apply_bankroll_ledger(
            vec![candidate(opened, opened + Duration::days(1), 10.0, 3.0)],
            4.0,
            5.0,
            true,
        );
        assert_eq!(result.summary.trades_opened, 0);
        assert_eq!(result.summary.trades_skipped_insufficient_cash, 1);
        assert_eq!(result.summary.final_equity, 4.0);
        assert_eq!(result.skips[0].reason, "insufficient_cash");
    }

    #[test]
    fn cash_between_minimum_and_desired_scales_trade() {
        let opened = "2026-04-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let result = apply_bankroll_ledger(
            vec![candidate(opened, opened + Duration::days(1), 10.0, 4.0)],
            6.0,
            5.0,
            true,
        );
        assert_eq!(result.summary.trades_opened, 1);
        assert_eq!(result.summary.trades_scaled, 1);
        assert_eq!(result.trades[0].scale_factor, 0.6);
        assert_eq!(result.trades[0].allocated_stake_usd, 6.0);
        assert_eq!(result.trades[0].pnl, 2.4);
        assert_eq!(result.summary.final_equity, 8.4);
    }

    #[test]
    fn scale_down_can_be_disabled() {
        let opened = "2026-04-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let result = apply_bankroll_ledger(
            vec![candidate(opened, opened + Duration::days(1), 10.0, 4.0)],
            6.0,
            5.0,
            false,
        );
        assert_eq!(result.summary.trades_opened, 0);
        assert_eq!(result.summary.trades_skipped_insufficient_cash, 1);
    }

    #[test]
    fn drawdown_and_utilization_are_recorded() {
        let opened = "2026-04-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let result = apply_bankroll_ledger(
            vec![
                candidate(opened, opened + Duration::hours(12), 10.0, -10.0),
                candidate(
                    opened + Duration::days(1),
                    opened + Duration::days(2),
                    10.0,
                    5.0,
                ),
            ],
            100.0,
            5.0,
            true,
        );
        assert!(result.summary.max_drawdown > 0.0);
        assert!(result.summary.max_locked_capital > 0.0);
        assert!(result.summary.max_capital_utilization > 0.0);
        assert!(result.summary.avg_capital_utilization > 0.0);
    }
}
