use std::{collections::HashMap, time::Duration};

use serde::Deserialize;

use eyre::{Context, Result};
use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use strum::{AsRefStr, EnumString};

// Configuration structs
#[derive(Deserialize)]
pub struct Conf {
    pub metrics: Vec<MetricType>,

    #[serde(flatten)]
    pub global_config: GlobalConfig,
}

#[derive(Deserialize)]
pub struct GlobalConfig {
    #[serde(with = "humantime_serde")]
    #[serde(default = "default_metric_clear_timeout")]
    pub metric_clear_timeout: Duration,
}

fn default_metric_clear_timeout() -> Duration {
    Duration::from_secs(10)
}

#[derive(Deserialize, AsRefStr, Clone, Debug)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum MetricType {
    Dns(DnsConfig),
    Icmp(IcmpConfig),
    Hls(HlsConfig),
}

#[derive(Deserialize, Clone, Debug)]
pub struct HlsConfig {
    #[serde(flatten)]
    pub common_config: MetricConfig,

    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DnsConfig {
    #[serde(flatten)]
    pub common_config: MetricConfig,
}

#[derive(Deserialize, Clone, Debug)]
pub struct IcmpConfig {
    #[serde(flatten)]
    pub common_config: MetricConfig,
}

#[derive(Deserialize, Clone, Debug)]
pub struct MetricConfig {
    #[serde(default)]
    pub prefix: String,
    pub endpoint: String,
    #[serde(with = "humantime_serde")]
    pub frequency: Duration,

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
            .merge(Yaml::file("Metrics.yaml"))
            .extract()
            .context("Unable to read config file")
    }
}
