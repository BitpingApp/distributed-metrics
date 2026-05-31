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
    /// How long to keep metrics after a scrape. Set to `null` or omit to
    /// disable clearing (metrics persist until the process restarts).
    /// Only safe to disable when using `label_whitelist` to bound cardinality.
    #[serde(default = "default_metric_clear_timeout")]
    #[serde(with = "humantime_serde::option")]
    pub metric_clear_timeout: Option<Duration>,

    /// Enable the /metrics scrape endpoint (default: true)
    #[serde(default = "default_true")]
    pub scrape_enabled: bool,

    /// Remote write destinations
    #[serde(default)]
    pub remote_write: Vec<RemoteWriteDestination>,
}

fn default_metric_clear_timeout() -> Option<Duration> {
    Some(Duration::from_secs(10))
}

fn default_true() -> bool {
    true
}

fn default_remote_write_interval() -> Duration {
    Duration::from_secs(15)
}

fn default_remote_write_timeout() -> Duration {
    Duration::from_secs(30)
}

#[derive(Deserialize, Clone, Debug)]
pub struct RemoteWriteDestination {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Custom HTTP headers (e.g., bearer tokens, API keys).
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(with = "humantime_serde")]
    #[serde(default = "default_remote_write_interval")]
    pub interval: Duration,
    #[serde(with = "humantime_serde")]
    #[serde(default = "default_remote_write_timeout")]
    pub timeout: Duration,
}

#[derive(Deserialize, AsRefStr, Clone, Debug)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum MetricType {
    Dns(DnsConfig),
    Icmp(IcmpConfig),
    Hls(HlsConfig),
    Http(HttpConfig),
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Deserialize, AsRefStr, Clone, Debug)]
pub enum HttpMethod {
    GET,
    POST,
    PUT,
    PATCH,
    OPTIONS,
    DELETE,
    HEAD,
}

#[derive(Deserialize, Clone, Debug)]
pub struct HttpConfig {
    #[serde(flatten)]
    pub common_config: MetricConfig,

    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub method: HttpMethod,
    pub body: Option<String>,
    pub regex: Option<String>,
    pub status_codes: Option<Vec<u16>>,

    /// When true, request the node to capture TLS certificate metadata
    /// (issuer, subject, expiry). Enables the http_ssl_* metric family.
    #[serde(default)]
    pub ssl_info: Option<bool>,

    /// Transport selection. Defaults to TCP (HTTP/1.1 + HTTP/2). Set to
    /// "AUTO" to enable HTTP/3 with fallback (populates http_fallback_total),
    /// or "QUIC" for HTTP/3 only.
    #[serde(default)]
    pub transport: Option<String>,
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
    #[serde(default)]
    pub lookup_type: LookupTypes,
    #[serde(default)]
    pub dns_servers: Option<Vec<String>>,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Deserialize, AsRefStr, Clone, Debug, Default)]
pub enum LookupTypes {
    #[default]
    IP,
    MX,
    SOA,
    NS,
    TXT,
    SRV,
    TLSA,
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
    #[serde(default)]
    pub name: Option<String>,
    pub endpoint: String,
    #[serde(with = "humantime_serde")]
    pub frequency: Duration,

    pub network: Option<NetworkCriteria>,

    /// Optional label whitelist. When set, only labels whose keys appear in
    /// this list are kept on recorded metrics. Unlisted labels are dropped
    /// before recording, which reduces cardinality.
    /// Example: `["country_code", "endpoint"]`
    #[serde(default)]
    pub label_whitelist: Option<Vec<String>>,
}

impl MetricConfig {
    /// Filter a labels map to only include whitelisted keys.
    /// If no whitelist is configured, all labels pass through.
    pub fn filter_labels<'a>(&self, labels: &mut HashMap<&'a str, String>) {
        if let Some(whitelist) = &self.label_whitelist {
            labels.retain(|k, _| whitelist.iter().any(|w| w == k));
        }
    }
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
    #[serde(default)]
    pub proxy: Policy,
    #[serde(default)]
    pub mobile: Policy,
    #[serde(default)]
    pub residential: Policy,
    pub country_code: Option<keshvar::Alpha3>,
    pub continent_code: Option<ContinentCode>,
    pub isp_regex: Option<String>,
    pub node_id: Option<String>,
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
