use std::env;
use std::str::FromStr;

use alloy::signers::local::{LocalSigner, PrivateKeySigner};
use alloy::signers::Signer as _;
use anyhow::{Context, Result};
use polymarket_client_sdk_v2::auth::{Credentials, Normal, Uuid};
use polymarket_client_sdk_v2::clob::types::request::BalanceAllowanceRequest;
use polymarket_client_sdk_v2::clob::types::response::BalanceAllowanceResponse;
use polymarket_client_sdk_v2::clob::types::{AssetType, OrderType, Side, SignatureType};
use polymarket_client_sdk_v2::clob::{Client as ClobClient, Config as ClobConfig};
use polymarket_client_sdk_v2::types::{Address as PolyAddress, Decimal, U256};
use polymarket_client_sdk_v2::POLYGON;
use tracing::{error, info, instrument};

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

    #[instrument(
        name = "submit_no_buy_order",
        skip(self),
        fields(token_id = %token_id, price, shares)
    )]
    pub async fn submit_no_buy_order(
        &self,
        token_id: &str,
        price: f64,
        shares: f64,
    ) -> Result<String> {
        info!("submitting live NO buy order");

        let client = self.authenticate().await.map_err(|error| {
            error!(error = %error, "failed to authenticate before order submission");
            error
        })?;
        let signer = load_signer().map_err(|error| {
            error!(error = %error, "failed to load signer before order submission");
            error
        })?;
        let token_id = U256::from_str(token_id)
            .map_err(|error| {
                error!(error = %error, raw_token_id = token_id, "failed to parse token id");
                error
            })
            .with_context(|| format!("failed to parse token id {}", token_id))?;

        let order = client
            .limit_order()
            .token_id(token_id)
            .order_type(OrderType::GTC)
            .price(decimal_from_f64(price)?)
            .size(decimal_from_f64(shares)?)
            .side(Side::Buy)
            .build()
            .await
            .map_err(|error| {
                error!(error = %error, instrument = %token_id, "failed to build live order");
                error
            })
            .with_context(|| format!("failed to build live order for token {}", token_id))?;

        let signed = client
            .sign(&signer, order)
            .await
            .map_err(|error| {
                error!(error = %error, instrument = %token_id, "failed to sign live order");
                error
            })
            .with_context(|| format!("failed to sign live order for token {}", token_id))?;

        let response = client
            .post_order(signed)
            .await
            .map_err(|error| {
                error!(error = %error, instrument = %token_id, "failed to submit live order");
                error
            })
            .with_context(|| format!("failed to submit live order for token {}", token_id))?;

        if !response.success {
            let message = response
                .error_msg
                .unwrap_or_else(|| "unknown exchange error".to_string());
            error!(
                instrument = %token_id,
                order_id = %response.order_id,
                error_message = %message,
                "live order rejected by exchange"
            );
            anyhow::bail!("live order rejected for token {}: {}", token_id, message);
        }

        info!(instrument = %token_id, order_id = %response.order_id, "live order submitted");
        Ok(response.order_id)
    }

    pub async fn get_balance_allowance(&self) -> Result<BalanceAllowanceResponse> {
        let client = self.authenticate().await?;
        let request = BalanceAllowanceRequest::builder()
            .asset_type(AssetType::Collateral)
            .build();
        client
            .balance_allowance(request)
            .await
            .context("failed to fetch wallet balance/allowance")
    }

    pub fn wallet_address(&self) -> Result<String> {
        Ok(load_signer()?.address().to_string())
    }

    async fn authenticate(
        &self,
    ) -> Result<ClobClient<polymarket_client_sdk_v2::auth::state::Authenticated<Normal>>> {
        let signer = load_signer()?;
        let config = ClobConfig::builder().use_server_time(true).build();
        let base = ClobClient::new(&self.clob_url, config)
            .context("failed to build Polymarket CLOB client")?;
        let credentials = self.fetch_credentials(&base, &signer).await?;

        let mut auth = base
            .authentication_builder(&signer)
            .credentials(credentials)
            // Match existing proxy/Safe users by default; new API users should set Poly1271.
            .signature_type(load_signature_type().unwrap_or(SignatureType::GnosisSafe));

        if let Some(funder) = load_funder_address()? {
            auth = auth.funder(funder);
        }

        auth.authenticate()
            .await
            .context("failed to authenticate Polymarket CLOB client")
    }

    async fn fetch_credentials(
        &self,
        base: &ClobClient<polymarket_client_sdk_v2::auth::state::Unauthenticated>,
        signer: &PrivateKeySigner,
    ) -> Result<Credentials> {
        if let Some(credentials) = load_api_credentials()? {
            return Ok(credentials);
        }

        match base.create_api_key(signer, None).await {
            Ok(credentials) => Ok(credentials),
            Err(create_error) => match base.derive_api_key(signer, None).await {
                Ok(credentials) => Ok(credentials),
                Err(derive_error) => Err(anyhow::anyhow!(
                    "failed to obtain Polymarket API credentials for signer {}.\ncreate_api_key error: {}\nderive_api_key error: {}",
                    signer.address(),
                    create_error,
                    derive_error
                )),
            },
        }
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

fn load_api_credentials() -> Result<Option<Credentials>> {
    let api_key = first_non_empty_env(&["POLYMARKET_API_KEY", "API_KEY", "api_key"]);
    let api_secret = first_non_empty_env(&["POLYMARKET_API_SECRET", "API_SECRET", "api_secret"]);
    let api_passphrase = first_non_empty_env(&[
        "POLYMARKET_API_PASSPHRASE",
        "API_PASSPHRASE",
        "api_passphrase",
    ]);

    let all_blank = api_key.is_none() && api_secret.is_none() && api_passphrase.is_none();
    if all_blank {
        return Ok(None);
    }

    if api_key.is_none() || api_secret.is_none() || api_passphrase.is_none() {
        anyhow::bail!(
            "API credentials must all be set together. Supported names: POLYMARKET_API_KEY/API_KEY, POLYMARKET_API_SECRET/API_SECRET, POLYMARKET_API_PASSPHRASE/API_PASSPHRASE"
        );
    }

    let api_key = Uuid::parse_str(api_key.as_deref().unwrap().trim())
        .context("failed to parse API key as UUID")?;
    Ok(Some(Credentials::new(
        api_key,
        api_secret.unwrap().trim().to_string(),
        api_passphrase.unwrap().trim().to_string(),
    )))
}

fn load_signature_type() -> Option<SignatureType> {
    match first_non_empty_env(&["POLYMARKET_SIGNATURE_TYPE", "SIGNATURE_TYPE"]).as_deref() {
        Some("0") | Some("eoa") | Some("EOA") => Some(SignatureType::Eoa),
        Some("1") | Some("proxy") | Some("PROXY") => Some(SignatureType::Proxy),
        Some("2") | Some("safe") | Some("SAFE") | Some("gnosis_safe") => {
            Some(SignatureType::GnosisSafe)
        }
        Some("3") | Some("poly_1271") | Some("POLY_1271") | Some("poly1271") | Some("POLY1271")
        | Some("erc1271") | Some("ERC1271") => Some(SignatureType::Poly1271),
        Some(_) | None => None,
    }
}

fn load_funder_address() -> Result<Option<PolyAddress>> {
    let Some(raw) = first_non_empty_env(&["POLYMARKET_FUNDER", "FUNDER_ADDRESS"]) else {
        return Ok(None);
    };

    let funder = PolyAddress::from_str(raw.trim())
        .context("failed to parse funder address from POLYMARKET_FUNDER/FUNDER_ADDRESS")?;
    Ok(Some(funder))
}

fn first_non_empty_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}
