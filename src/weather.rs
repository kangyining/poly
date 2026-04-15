use anyhow::Result;
use chrono::NaiveDate;

#[derive(Debug, Clone, Copy)]
pub struct Forecast {
    pub target_date: NaiveDate,
    pub forecast_high_f: f64,
}

pub trait ForecastProvider {
    fn get_forecast(&self, target_date: NaiveDate) -> Result<Forecast>;
}

#[derive(Debug, Clone, Copy)]
pub struct StaticForecastProvider {
    forecast_high_f: f64,
}

impl StaticForecastProvider {
    pub fn new(forecast_high_f: f64) -> Self {
        Self { forecast_high_f }
    }
}

impl ForecastProvider for StaticForecastProvider {
    fn get_forecast(&self, target_date: NaiveDate) -> Result<Forecast> {
        Ok(Forecast {
            target_date,
            forecast_high_f: self.forecast_high_f,
        })
    }
}
