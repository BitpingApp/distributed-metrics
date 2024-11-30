use serde::Deserialize;

use eyre::{Context, Result};
use figment::{
    providers::{Env, Format, Toml, Yaml},
    Figment,
};
use strum::{AsRefStr, EnumString};

// Configuration structs
#[derive(Deserialize)]
pub struct Conf {
    pub metric: Vec<MetricConfig>,
}

#[derive(Deserialize, EnumString, AsRefStr, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
    #[strum(serialize = "dns")]
    Dns,
}

#[derive(Deserialize, Clone, Debug)]
pub struct MetricConfig {
    pub prefix: String,
    #[serde(rename = "type")]
    pub metric_type: MetricType,
    pub endpoint: String,
    pub frequency_ms: u64,

    pub network: Option<NetworkCriteria>,
}

#[derive(Deserialize, EnumString, AsRefStr, Clone, Default, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    #[default]
    Allowed,
    Denied,
    Required,
}

#[derive(Deserialize, EnumString, AsRefStr, Clone, Debug)]
pub enum ContinentCode {
    AF,
    AN,
    AS,
    EU,
    NA,
    OC,
    SA,
}

#[derive(Deserialize, Clone, Debug)]
pub struct NetworkCriteria {
    pub proxy: Policy,
    pub mobile: Policy,
    pub residential: Policy,
    pub country_code: Option<keshvar::Alpha3>,
    pub continent_code: Option<ContinentCode>,
}

impl Conf {
    pub fn new() -> Result<Self> {
        Figment::new()
            .join(Env::prefixed("BITPING_"))
            .merge(Toml::file("Metrics.toml"))
            .merge(Yaml::file("Metrics.yaml"))
            .extract()
            .context("Unable to read config file")
    }
}
