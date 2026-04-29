use std::fs::OpenOptions;
use std::io::Write;

use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};

use crate::config::AppConfig;
use crate::core::state::{
    PortfolioPosition, PortfolioState, SharedState, TradeRationale,
};
use crate::market::PolymarketClient;

use super::{ExecutedTrade, ExecutionReport};

const PERFORMANCE_CSV_HEADER: &str = "sequence,logged_at,action,run_id,decision_id,event_slug,target_date,market_id,bin_label,token_id,order_id,resolution_bin_label,is_paper,status,shares,entry_price,stake_usd,payout_usd,pnl_usd,cash_before_usd,cash_after_usd,open_exposure_after_usd,realized_pnl_after_usd,equity_after_usd,starting_balance_usd,notes\n";

pub struct PortfolioService {
    config: AppConfig,
    state: SharedState,
}

impl PortfolioService {
    pub fn new(config: AppConfig, state: SharedState) -> Self {
        Self { config, state }
    }

    pub async fn initialize(&self) -> Result<()> {
        let portfolio = self.load_or_init_portfolio()?;
        self.persist_and_sync(&portfolio).await
    }

    pub async fn record_execution(&self, report: &ExecutionReport) -> Result<()> {
        let mut portfolio = self.load_or_init_portfolio()?;
        self.reconcile_positions(&mut portfolio).await?;
        self.apply_execution_report(&mut portfolio, report)?;
        self.persist_and_sync(&portfolio).await
    }

    async fn reconcile_positions(&self, portfolio: &mut PortfolioState) -> Result<()> {
        let today = Utc::now().date_naive();
        let client = PolymarketClient::new(&self.config.gamma_url, &self.config.clob_url);

        for idx in 0..portfolio.positions.len() {
            if portfolio.positions[idx].status != "open" {
                continue;
            }

            let Ok(target_date) =
                NaiveDate::parse_from_str(&portfolio.positions[idx].target_date, "%Y-%m-%d")
            else {
                continue;
            };
            if target_date > today {
                continue;
            }

            let event = client
                .get_event_by_slug(&portfolio.positions[idx].event_slug)
                .await
                .with_context(|| {
                    format!(
                        "failed to fetch event {}",
                        portfolio.positions[idx].event_slug
                    )
                })?;
            let Some(winning_bin) = event.winning_bin_label() else {
                continue;
            };

            let cash_before = portfolio.cash_usd;
            let now = Utc::now().to_rfc3339();
            let (
                run_id,
                decision_id,
                event_slug,
                target_date,
                market_id,
                bin_label,
                token_id,
                order_id,
                is_paper,
                status,
                shares,
                entry_price,
                stake_usd,
                payout_usd,
                pnl_usd,
            ) = {
                let position = &mut portfolio.positions[idx];
                let payout_usd = if position.bin_label == winning_bin {
                    0.0
                } else {
                    position.shares
                };
                let pnl_usd = payout_usd - position.stake_usd;
                if position.is_paper {
                    portfolio.cash_usd += payout_usd;
                }
                position.status = "closed".to_string();
                position.pnl = Some(round4(pnl_usd));
                position.payout_usd = Some(round4(payout_usd));
                position.resolution_bin_label = Some(winning_bin.clone());
                position.settled_at = Some(now.clone());
                (
                    position.run_id.clone(),
                    position.decision_id.clone(),
                    position.event_slug.clone(),
                    position.target_date.clone(),
                    position.market_id.clone(),
                    position.bin_label.clone(),
                    position.token_id.clone(),
                    position.order_id.clone(),
                    position.is_paper,
                    position.status.clone(),
                    position.shares,
                    position.entry_price,
                    position.stake_usd,
                    payout_usd,
                    pnl_usd,
                )
            };
            portfolio.refresh_summary();
            portfolio.last_updated_at = now.clone();

            let action = crate::core::state::PerformanceActionState {
                sequence: 0,
                logged_at: parse_datetime_opt(Some(&now)),
                action: "SETTLE".to_string(),
                run_id,
                decision_id,
                event_slug: Some(event_slug),
                target_date: NaiveDate::parse_from_str(&target_date, "%Y-%m-%d").ok(),
                market_id: Some(market_id),
                bin_label: Some(bin_label),
                token_id: Some(token_id),
                order_id,
                resolution_bin_label: Some(winning_bin),
                is_paper,
                status: Some(status),
                shares,
                entry_price,
                stake_usd,
                payout_usd: Some(round4(payout_usd)),
                pnl_usd: Some(round4(pnl_usd)),
                cash_before_usd: Some(round4(cash_before)),
                cash_after_usd: Some(round4(portfolio.cash_usd)),
                open_exposure_after_usd: Some(round4(portfolio.open_exposure_usd())),
                realized_pnl_after_usd: Some(round4(portfolio.realized_pnl_usd)),
                equity_after_usd: Some(round4(portfolio.equity_usd)),
                starting_balance_usd: Some(round4(portfolio.starting_balance_usd)),
                notes: Some("resolved market".to_string()),
            };
            self.append_action(portfolio, action)?;
        }

        Ok(())
    }

    fn apply_execution_report(
        &self,
        portfolio: &mut PortfolioState,
        report: &ExecutionReport,
    ) -> Result<()> {
        for trade in &report.trades {
            if portfolio
                .positions
                .iter()
                .any(|position| position.token_id == trade.token_id && position.status == "open")
            {
                continue;
            }

            let cash_before = portfolio.cash_usd;
            if trade.is_paper && portfolio.cash_usd + 1e-9 < trade.stake_usd {
                let action = crate::core::state::PerformanceActionState {
                    sequence: 0,
                    logged_at: Some(Utc::now()),
                    action: "SKIP_INSUFFICIENT_CASH".to_string(),
                    run_id: Some(report.run_id.clone()),
                    decision_id: Some(trade.decision_id.clone()),
                    event_slug: Some(report.event_slug.clone()),
                    target_date: NaiveDate::parse_from_str(&report.target_date, "%Y-%m-%d").ok(),
                    market_id: Some(trade.market_id.clone()),
                    bin_label: Some(trade.bin_label.clone()),
                    token_id: Some(trade.token_id.clone()),
                    order_id: trade.order_id.clone(),
                    resolution_bin_label: None,
                    is_paper: trade.is_paper,
                    status: Some("skipped".to_string()),
                    shares: trade.shares,
                    entry_price: trade.effective_ask,
                    stake_usd: trade.stake_usd,
                    payout_usd: None,
                    pnl_usd: None,
                    cash_before_usd: Some(round4(cash_before)),
                    cash_after_usd: Some(round4(portfolio.cash_usd)),
                    open_exposure_after_usd: Some(round4(portfolio.open_exposure_usd())),
                    realized_pnl_after_usd: Some(round4(portfolio.realized_pnl_usd)),
                    equity_after_usd: Some(round4(portfolio.equity_usd)),
                    starting_balance_usd: Some(round4(portfolio.starting_balance_usd)),
                    notes: Some("paper starting balance exhausted".to_string()),
                };
                self.append_action(portfolio, action)?;
                continue;
            }

            if trade.is_paper {
                portfolio.cash_usd -= trade.stake_usd;
            }
            portfolio
                .positions
                .push(PortfolioPosition::from_executed_trade(report, trade));
            portfolio.refresh_summary();
            portfolio.last_updated_at = Utc::now().to_rfc3339();

            let action = crate::core::state::PerformanceActionState {
                sequence: 0,
                logged_at: parse_datetime_opt(Some(&portfolio.last_updated_at)),
                action: "BUY_NO".to_string(),
                run_id: Some(report.run_id.clone()),
                decision_id: Some(trade.decision_id.clone()),
                event_slug: Some(report.event_slug.clone()),
                target_date: NaiveDate::parse_from_str(&report.target_date, "%Y-%m-%d").ok(),
                market_id: Some(trade.market_id.clone()),
                bin_label: Some(trade.bin_label.clone()),
                token_id: Some(trade.token_id.clone()),
                order_id: trade.order_id.clone(),
                resolution_bin_label: None,
                is_paper: trade.is_paper,
                status: Some("open".to_string()),
                shares: trade.shares,
                entry_price: trade.effective_ask,
                stake_usd: trade.stake_usd,
                payout_usd: None,
                pnl_usd: None,
                cash_before_usd: Some(round4(cash_before)),
                cash_after_usd: Some(round4(portfolio.cash_usd)),
                open_exposure_after_usd: Some(round4(portfolio.open_exposure_usd())),
                realized_pnl_after_usd: Some(round4(portfolio.realized_pnl_usd)),
                equity_after_usd: Some(round4(portfolio.equity_usd)),
                starting_balance_usd: Some(round4(portfolio.starting_balance_usd)),
                notes: Some(if trade.is_paper {
                    "paper position opened".to_string()
                } else {
                    "live position recorded".to_string()
                }),
            };
            self.append_action(portfolio, action)?;
        }
        Ok(())
    }

    async fn persist_and_sync(&self, portfolio: &PortfolioState) -> Result<()> {
        self.write_portfolio_state(portfolio)?;
        self.write_open_positions_export(portfolio)?;
        let mut state = self.state.write().await;
        state.performance.starting_balance_usd = portfolio.starting_balance_usd;
        state.performance.cash_usd = portfolio.cash_usd;
        state.performance.open_exposure_usd = portfolio.open_exposure_usd();
        state.performance.realized_pnl_usd = portfolio.realized_pnl_usd;
        state.performance.equity_usd = portfolio.equity_usd;
        state.performance.open_position_count = portfolio
            .positions
            .iter()
            .filter(|position| position.status == "open")
            .count();
        state.performance.closed_position_count = portfolio
            .positions
            .iter()
            .filter(|position| position.status == "closed")
            .count();
        state.performance.action_count = portfolio.action_history.len() as u64;
        state.performance.action_history = portfolio.action_history.clone();
        state.performance.last_action = state.performance.action_history.last().cloned();
        state.performance.last_updated_at = parse_datetime_opt(Some(&portfolio.last_updated_at));
        Ok(())
    }

    fn load_or_init_portfolio(&self) -> Result<PortfolioState> {
        if self.config.paper_portfolio_path.exists() {
            let text = std::fs::read_to_string(&self.config.paper_portfolio_path)?;
            let mut portfolio: PortfolioState = serde_json::from_str(&text)?;
            if portfolio.next_sequence == 0 {
                portfolio.next_sequence = 1;
            }
            portfolio.refresh_summary();
            return Ok(portfolio);
        }

        let imported_positions = self.load_existing_open_positions()?;
        let imported_paper_stake: f64 = imported_positions
            .iter()
            .filter(|position| position.is_paper && position.status == "open")
            .map(|position| position.stake_usd)
            .sum();
        let mut portfolio = PortfolioState {
            starting_balance_usd: self.config.paper_starting_balance_usd,
            cash_usd: self.config.paper_starting_balance_usd - imported_paper_stake,
            realized_pnl_usd: 0.0,
            equity_usd: 0.0,
            last_updated_at: Utc::now().to_rfc3339(),
            next_sequence: 1,
            positions: imported_positions,
            action_history: Vec::new(),
        };
        portfolio.refresh_summary();
        let action = crate::core::state::PerformanceActionState {
            sequence: 0,
            logged_at: parse_datetime_opt(Some(&portfolio.last_updated_at)),
            action: "START".to_string(),
            run_id: None,
            decision_id: None,
            event_slug: None,
            target_date: None,
            market_id: None,
            bin_label: None,
            token_id: None,
            order_id: None,
            resolution_bin_label: None,
            is_paper: true,
            status: Some("initialized".to_string()),
            shares: 0.0,
            entry_price: 0.0,
            stake_usd: 0.0,
            payout_usd: None,
            pnl_usd: None,
            cash_before_usd: Some(round4(portfolio.cash_usd)),
            cash_after_usd: Some(round4(portfolio.cash_usd)),
            open_exposure_after_usd: Some(round4(portfolio.open_exposure_usd())),
            realized_pnl_after_usd: Some(round4(portfolio.realized_pnl_usd)),
            equity_after_usd: Some(round4(portfolio.equity_usd)),
            starting_balance_usd: Some(round4(portfolio.starting_balance_usd)),
            notes: Some(format!(
                "paper portfolio initialized; imported {} existing open positions",
                portfolio
                    .positions
                    .iter()
                    .filter(|position| position.status == "open")
                    .count()
            )),
        };
        self.append_action(&mut portfolio, action)?;
        Ok(portfolio)
    }

    fn load_existing_open_positions(&self) -> Result<Vec<PortfolioPosition>> {
        if !self.config.open_positions_path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&self.config.open_positions_path)?;
        let positions = serde_json::from_str(&text)?;
        Ok(positions)
    }

    fn append_action(
        &self,
        portfolio: &mut PortfolioState,
        mut action: crate::core::state::PerformanceActionState,
    ) -> Result<()> {
        self.ensure_csv_header()?;
        action.sequence = portfolio.next_sequence;
        portfolio.next_sequence += 1;
        portfolio.action_history.push(action.clone());

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.config.performance_csv_path)?;
        let line = [
            action.sequence.to_string(),
            action
                .logged_at
                .map(|value| value.to_rfc3339())
                .unwrap_or_default(),
            action.action.clone(),
            action.run_id.clone().unwrap_or_default(),
            action.decision_id.clone().unwrap_or_default(),
            action.event_slug.clone().unwrap_or_default(),
            action
                .target_date
                .map(|value| value.to_string())
                .unwrap_or_default(),
            action.market_id.clone().unwrap_or_default(),
            action.bin_label.clone().unwrap_or_default(),
            action.token_id.clone().unwrap_or_default(),
            action.order_id.clone().unwrap_or_default(),
            action.resolution_bin_label.clone().unwrap_or_default(),
            action.is_paper.to_string(),
            action.status.clone().unwrap_or_default(),
            fmt_f64(action.shares),
            fmt_f64(action.entry_price),
            fmt_f64(action.stake_usd),
            fmt_opt_f64(action.payout_usd),
            fmt_opt_f64(action.pnl_usd),
            fmt_opt_f64(action.cash_before_usd),
            fmt_opt_f64(action.cash_after_usd),
            fmt_opt_f64(action.open_exposure_after_usd),
            fmt_opt_f64(action.realized_pnl_after_usd),
            fmt_opt_f64(action.equity_after_usd),
            fmt_opt_f64(action.starting_balance_usd),
            action.notes.clone().unwrap_or_default(),
        ]
        .into_iter()
        .map(|value| csv_escape(&value))
        .collect::<Vec<_>>()
        .join(",");
        writeln!(file, "{line}")?;
        Ok(())
    }

    fn ensure_csv_header(&self) -> Result<()> {
        if let Some(parent) = self.config.performance_csv_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !self.config.performance_csv_path.exists() {
            std::fs::write(&self.config.performance_csv_path, PERFORMANCE_CSV_HEADER)?;
        }
        Ok(())
    }

    fn write_portfolio_state(&self, portfolio: &PortfolioState) -> Result<()> {
        if let Some(parent) = self.config.paper_portfolio_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            &self.config.paper_portfolio_path,
            serde_json::to_string_pretty(portfolio)?,
        )?;
        Ok(())
    }

    fn write_open_positions_export(&self, portfolio: &PortfolioState) -> Result<()> {
        if let Some(parent) = self.config.open_positions_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let open_positions: Vec<_> = portfolio
            .positions
            .iter()
            .filter(|position| position.status == "open")
            .cloned()
            .collect();
        std::fs::write(
            &self.config.open_positions_path,
            serde_json::to_string_pretty(&open_positions)?,
        )?;
        Ok(())
    }
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn fmt_f64(value: f64) -> String {
    format!("{value:.4}")
}

fn fmt_opt_f64(value: Option<f64>) -> String {
    value.map(fmt_f64).unwrap_or_default()
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn parse_datetime_opt(value: Option<&str>) -> Option<chrono::DateTime<Utc>> {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

impl PortfolioPosition {
    fn from_executed_trade(report: &ExecutionReport, trade: &ExecutedTrade) -> Self {
        Self {
            token_id: trade.token_id.clone(),
            market_id: trade.market_id.clone(),
            bin_label: trade.bin_label.clone(),
            event_slug: report.event_slug.clone(),
            target_date: report.target_date.clone(),
            entry_price: trade.effective_ask,
            shares: trade.shares,
            stake_usd: trade.stake_usd,
            order_id: trade.order_id.clone(),
            status: "open".to_string(),
            pnl: None,
            opened_at: trade.executed_at.clone(),
            settled_at: None,
            is_paper: trade.is_paper,
            strategy_name: trade.strategy_name.clone(),
            backtest_id: None,
            trade_rationale: Some(TradeRationale {
                ev_no: trade.ev_no,
                p_model: trade.p_model,
                fee_rate: trade.fee_rate,
                effective_ask: trade.effective_ask,
            }),
            run_id: Some(report.run_id.clone()),
            decision_id: Some(trade.decision_id.clone()),
            payout_usd: None,
            resolution_bin_label: None,
        }
    }
}
