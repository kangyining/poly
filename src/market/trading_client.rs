use std::env;
use std::str::FromStr;

use alloy::signers::local::{LocalSigner, PrivateKeySigner};
use alloy::signers::Signer as _;
use anyhow::{Context, Result};
use polymarket_client_sdk::auth::Normal;
use polymarket_client_sdk::clob::types::{OrderType, Side, SignatureType};
use polymarket_client_sdk::clob::{Client as ClobClient, Config as ClobConfig};
use polymarket_client_sdk::types::{Address as PolyAddress, Decimal};
use polymarket_client_sdk::POLYGON;

#[derive(Clone)]
pub struct TradingClient {
    clob_url: String,
}

impl TradingClient {
    pub fn new(clob_url: &str) -> Self {
        Self {
            clob_url: clob_url.to_string(),
        }
    }

    pub async fn submit_no_buy_order(
        &self,
        token_id: &str,
        price: f64,
        shares: f64,
    ) -> Result<String> {
        let client = self.authenticate().await?;
        let signer = load_signer()?;

        let order = client
            .limit_order()
            .token_id(token_id)
            .order_type(OrderType::GTC)
            .price(decimal_from_f64(price)?)
            .size(decimal_from_f64(shares)?)
            .side(Side::Buy)
            .build()
            .await
            .with_context(|| format!("failed to build live order for token {}", token_id))?;

        let signed = client
            .sign(&signer, order)
            .await
            .with_context(|| format!("failed to sign live order for token {}", token_id))?;

        let response = client
            .post_order(signed)
            .await
            .with_context(|| format!("failed to submit live order for token {}", token_id))?;

        if !response.success {
            let message = response
                .error_msg
                .unwrap_or_else(|| "unknown exchange error".to_string());
            anyhow::bail!("live order rejected for token {}: {}", token_id, message);
        }

        Ok(response.order_id)
    }

    async fn authenticate(
        &self,
    ) -> Result<ClobClient<polymarket_client_sdk::auth::state::Authenticated<Normal>>> {
        let signer = load_signer()?;
        let config = ClobConfig::builder().use_server_time(true).build();
        let base = ClobClient::new(&self.clob_url, config)
            .context("failed to build Polymarket CLOB client")?;

        let mut auth = base.authentication_builder(&signer);
        let sig_type = env::var("POLYMARKET_SIGNATURE_TYPE")
            .ok()
            .and_then(|v| v.parse::<u8>().ok());

        auth = match sig_type {
            Some(2) => auth.signature_type(SignatureType::GnosisSafe),
            Some(1) => {
                if let Ok(funder_raw) = env::var("POLYMARKET_FUNDER") {
                    let funder = PolyAddress::from_str(&funder_raw)
                        .context("failed to parse POLYMARKET_FUNDER")?;
                    auth.funder(funder).signature_type(SignatureType::Proxy)
                } else {
                    auth.signature_type(SignatureType::Proxy)
                }
            }
            _ => {
                if let Ok(funder_raw) = env::var("POLYMARKET_FUNDER") {
                    let funder = PolyAddress::from_str(&funder_raw)
                        .context("failed to parse POLYMARKET_FUNDER")?;
                    if funder == signer.address() {
                        auth.signature_type(SignatureType::Eoa)
                    } else {
                        auth.funder(funder).signature_type(SignatureType::Proxy)
                    }
                } else {
                    auth.signature_type(SignatureType::Eoa)
                }
            }
        };

        auth.authenticate()
            .await
            .context("failed to authenticate Polymarket CLOB client")
    }
}

fn load_signer() -> Result<PrivateKeySigner> {
    let private_key = env::var("POLYMARKET_PRIVATE_KEY")
        .context("POLYMARKET_PRIVATE_KEY must be set for live trading")?;
    LocalSigner::from_str(&private_key)
        .context("failed to parse POLYMARKET_PRIVATE_KEY")?
        .with_chain_id(Some(POLYGON))
        .pipe(Ok)
}

fn decimal_from_f64(value: f64) -> Result<Decimal> {
    Decimal::from_str(&value.to_string())
        .with_context(|| format!("failed to convert {value} into Decimal"))
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}
