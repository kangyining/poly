use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::info;

#[derive(Debug, Clone)]
pub struct Forecast {
    pub context_key: String,
    pub city_slug: String,
    pub target_date: NaiveDate,
    pub forecast_high_f: f64,
    pub lead_hours: f64,
    pub station_code: String,
}

#[derive(Debug, Clone, Copy)]
pub struct StaticForecastProvider {
    forecast_high_f: f64,
}

impl StaticForecastProvider {
    pub fn new(forecast_high_f: f64) -> Self {
        Self { forecast_high_f }
    }

    pub fn get_forecast(
        &self,
        context_key: &str,
        city_slug: &str,
        target_date: NaiveDate,
        station_code: &str,
    ) -> Result<Forecast> {
        let now = Utc::now();
        let close = market_close_utc(target_date);
        let lead_hours = (close - now).num_minutes() as f64 / 60.0;
        Ok(Forecast {
            context_key: context_key.to_string(),
            city_slug: city_slug.to_string(),
            target_date,
            forecast_high_f: self.forecast_high_f,
            lead_hours: lead_hours.max(0.0),
            station_code: station_code.to_string(),
        })
    }
}

pub struct NwsForecastProvider {
    http: Client,
    lat: f64,
    lon: f64,
}

#[derive(Deserialize)]
struct PointsResponse {
    properties: PointsProperties,
}

#[derive(Deserialize)]
struct PointsProperties {
    #[serde(rename = "forecastHourly")]
    forecast_hourly: String,
}

#[derive(Deserialize)]
struct HourlyResponse {
    properties: HourlyProperties,
}

#[derive(Deserialize)]
struct HourlyProperties {
    periods: Vec<HourlyPeriod>,
}

#[derive(Deserialize)]
struct HourlyPeriod {
    #[serde(rename = "startTime")]
    start_time: String,
    temperature: f64,
    #[serde(rename = "temperatureUnit")]
    temperature_unit: String,
}

impl NwsForecastProvider {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            http: Client::builder()
                .user_agent("polymarket-weather-bot/0.1 (contact: bot@example.com)")
                .build()
                .expect("failed to build HTTP client"),
            lat,
            lon,
        }
    }

    pub async fn get_forecast(
        &self,
        context_key: &str,
        city_slug: &str,
        target_date: NaiveDate,
        station_code: &str,
    ) -> Result<Forecast> {
        let points_url = format!(
            "https://api.weather.gov/points/{:.4},{:.4}",
            self.lat, self.lon
        );
        let points: PointsResponse = self
            .http
            .get(&points_url)
            .send()
            .await
            .context("NWS /points request failed")?
            .error_for_status()
            .context("NWS /points returned error status")?
            .json()
            .await
            .context("failed to parse NWS /points response")?;

        let hourly: HourlyResponse = self
            .http
            .get(&points.properties.forecast_hourly)
            .send()
            .await
            .context("NWS forecastHourly request failed")?
            .error_for_status()
            .context("NWS forecastHourly returned error status")?
            .json()
            .await
            .context("failed to parse NWS forecastHourly response")?;

        let mut max_temp_f: Option<f64> = None;

        for period in &hourly.properties.periods {
            let start: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(&period.start_time)
                .with_context(|| {
                    format!(
                        "failed to parse NWS period startTime: {}",
                        period.start_time
                    )
                })?;

            if start.date_naive() != target_date {
                continue;
            }

            let temp_f = if period.temperature_unit == "C" {
                period.temperature * 9.0 / 5.0 + 32.0
            } else {
                period.temperature
            };

            max_temp_f = Some(max_temp_f.unwrap_or(f64::NEG_INFINITY).max(temp_f));
        }

        let forecast_high_f = max_temp_f
            .ok_or_else(|| anyhow::anyhow!("no NWS periods found for {}", target_date))?;

        let now = Utc::now();
        let close = market_close_utc(target_date);
        let lead_hours = ((close - now).num_minutes() as f64 / 60.0).max(0.0);

        info!(
            target_date = %target_date,
            forecast_high_f,
            lead_hours,
            station_code,
            "NWS forecast fetched"
        );

        Ok(Forecast {
            context_key: context_key.to_string(),
            city_slug: city_slug.to_string(),
            target_date,
            forecast_high_f,
            lead_hours,
            station_code: station_code.to_string(),
        })
    }
}

pub fn market_close_utc(target_date: NaiveDate) -> DateTime<Utc> {
    let naive = target_date
        .and_hms_opt(12, 0, 0)
        .expect("12:00:00 is always valid");
    DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
}
