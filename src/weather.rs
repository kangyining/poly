use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::info;

const USE_STATIC_FORECAST: bool = false;
const STATIC_FORECAST_HIGH_F: f64 = 80.0;
const GFS_FORECAST_URL: &str = "https://api.open-meteo.com/v1/gfs";
const FORECAST_LATITUDE: f64 = 40.7769;
const FORECAST_LONGITUDE: f64 = -73.8740;
const FORECAST_TIMEZONE: &str = "America/New_York";

#[derive(Debug, Clone)]
pub struct Forecast {
    pub target_date: NaiveDate,
    pub forecast_high_f: f64,
    pub fetched_at: DateTime<Utc>,
    pub source: String,
}

#[async_trait]
pub trait ForecastProvider {
    async fn get_forecast(&self, target_date: NaiveDate) -> Result<Forecast>;
}

pub struct WeatherClient {
    provider: Box<dyn ForecastProvider>,
}

impl WeatherClient {
    pub fn new() -> Self {
        let provider: Box<dyn ForecastProvider> = if USE_STATIC_FORECAST {
            Box::new(StaticForecastProvider::new(STATIC_FORECAST_HIGH_F))
        } else {
            Box::new(GfsForecastProvider::new(
                GFS_FORECAST_URL,
                FORECAST_LATITUDE,
                FORECAST_LONGITUDE,
                FORECAST_TIMEZONE,
            ))
        };

        Self { provider }
    }

    pub async fn fetch_forecast(&self, target_date: NaiveDate) -> Result<Forecast> {
        self.provider.get_forecast(target_date).await
    }
}

#[derive(Debug, Clone)]
pub struct StaticForecastProvider {
    forecast_high_f: f64,
}

impl StaticForecastProvider {
    pub fn new(forecast_high_f: f64) -> Self {
        Self { forecast_high_f }
    }
}

#[async_trait]
impl ForecastProvider for StaticForecastProvider {
    async fn get_forecast(&self, target_date: NaiveDate) -> Result<Forecast> {
        Ok(Forecast {
            target_date,
            forecast_high_f: self.forecast_high_f,
            fetched_at: Utc::now(),
            source: "static".to_string(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct GfsForecastProvider {
    http: Client,
    base_url: String,
    latitude: f64,
    longitude: f64,
    timezone: String,
}

impl GfsForecastProvider {
    pub fn new(
        base_url: impl Into<String>,
        latitude: f64,
        longitude: f64,
        timezone: impl Into<String>,
    ) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.into(),
            latitude,
            longitude,
            timezone: timezone.into(),
        }
    }
}

#[async_trait]
impl ForecastProvider for GfsForecastProvider {
    async fn get_forecast(&self, target_date: NaiveDate) -> Result<Forecast> {
        let response = self
            .http
            .get(&self.base_url)
            .query(&[
                ("latitude", self.latitude.to_string()),
                ("longitude", self.longitude.to_string()),
                ("daily", "temperature_2m_max".to_string()),
                ("temperature_unit", "fahrenheit".to_string()),
                ("timezone", self.timezone.clone()),
                ("start_date", target_date.to_string()),
                ("end_date", target_date.to_string()),
            ])
            .send()
            .await
            .context("failed to query GFS forecast provider")?
            .error_for_status()
            .context("GFS forecast provider returned an error")?;

        let payload: OpenMeteoDailyForecast = response
            .json()
            .await
            .context("failed to deserialize GFS forecast response")?;

        let forecast_high_f = payload
            .daily
            .temperature_2m_max
            .first()
            .copied()
            .context("GFS response did not include a daily max temperature")?;

        info!(
            target_date = %target_date,
            forecast_high_f,
            latitude = self.latitude,
            longitude = self.longitude,
            timezone = %self.timezone,
            "fetched GFS forecast"
        );

        Ok(Forecast {
            target_date,
            forecast_high_f,
            fetched_at: Utc::now(),
            source: "open-meteo-gfs".to_string(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct OpenMeteoDailyForecast {
    daily: OpenMeteoDaily,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoDaily {
    temperature_2m_max: Vec<f64>,
}
