use super::{Collector, CollectorErrors};
use crate::config::{HttpConfig, LookupTypes};
use crate::types::{
    PerformHttpBodyConfiguration, PerformHttpBodyContinentCode, PerformHttpBodyCountryCode,
    PerformHttpBodyMobile, PerformHttpBodyProxy, PerformHttpBodyResidential, PerformHttpResponse,
    PerformHttpResponseResultsItemResult,
};
use crate::API_CLIENT;
use color_eyre::eyre::Result;
use geohash::Coord;
use metrics::{counter, gauge, histogram};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use tracing::{error, info, warn};

pub struct HttpCollector {
    config: &'static HttpConfig,
}

impl Collector for HttpCollector {
    type Config = HttpConfig;
    type Response = PerformHttpResponse;

    fn new(config: &'static HttpConfig) -> Self {
        Self { config }
    }

    fn register_metrics(&self) {
        let prefix = &self.config.common_config.prefix;

        metrics::describe_histogram!(
            format!("{}http_request_duration_ms", prefix),
            "HTTP request duration in milliseconds"
        );

        metrics::describe_gauge!(
            format!("{}http_status_code", prefix),
            "HTTP response status code"
        );

        metrics::describe_gauge!(
            format!("{}http_body_hash", prefix),
            "Hash of the HTTP response body"
        );

        metrics::describe_counter!(
            format!("{}http_request_success_total", prefix),
            "Total number of successful HTTP requests"
        );

        metrics::describe_counter!(
            format!("{}http_request_error_total", prefix),
            "Total number of failed HTTP requests"
        );

        metrics::describe_counter!(
            format!("{}http_request_total", prefix),
            "Total number of HTTP requests"
        );
    }

    async fn perform_request(&self) -> Result<Self::Response> {
        let country_code = self
            .config
            .common_config
            .network
            .as_ref()
            .and_then(|x| x.country_code)
            .map(|c| c.to_alpha2().to_string())
            .and_then(|x| PerformHttpBodyCountryCode::from_str(&x).ok());

        let continent_code = self
            .config
            .common_config
            .network
            .as_ref()
            .and_then(|x| x.continent_code.clone())
            .and_then(|c| PerformHttpBodyContinentCode::from_str(c.as_ref()).ok());

        let mobile = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.mobile.as_ref().to_uppercase())
            .and_then(|mo| PerformHttpBodyMobile::from_str(&mo).ok())
            .unwrap_or_default();

        let residential = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.residential.as_ref().to_uppercase())
            .and_then(|mo| PerformHttpBodyResidential::from_str(&mo).ok())
            .unwrap_or_default();

        let proxy = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.proxy.as_ref().to_uppercase())
            .and_then(|mo| PerformHttpBodyProxy::from_str(&mo).ok())
            .unwrap_or_default();

        let isp = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.isp_regex.clone())
            .unwrap_or_default();

        let node_id = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.node_id.clone())
            .unwrap_or_default();

        info!(?self.config.common_config, ?country_code, "Sending http request");

        let response = API_CLIENT
            .perform_http()
            .method(self.config.method.as_ref())
            .body_map(|body| {
                body.hostnames([self.config.common_config.endpoint.clone()])
                    .country_code(country_code)
                    .continent_code(continent_code)
                    .mobile(mobile)
                    .residential(residential)
                    .isp_regex(isp)
                    .node_id(node_id)
                    .proxy(proxy)
                    .configuration(Some(PerformHttpBodyConfiguration {
                        body: self.config.body.clone(),
                        headers: self.config.headers.clone(),
                        regex: self.config.regex.clone(),
                        return_body: Some(true),
                        status_codes: vec![],
                    }))
            })
            .send()
            .await?;

        Ok(response.into_inner())
    }

    fn get_frequency(&self) -> std::time::Duration {
        self.config.common_config.frequency
    }

    fn handle_response(&self, response: PerformHttpResponse) -> Result<(), CollectorErrors> {
        let endpoint = self
            .config
            .common_config
            .name
            .as_ref()
            .unwrap_or(&self.config.common_config.endpoint);

        let node_info = response
            .node_info
            .ok_or_else(|| CollectorErrors::MissingNodeInfo(endpoint.clone()))?;

        // Core labels - essential dimensions only
        let mut labels: HashMap<&str, String> = HashMap::from_iter([
            ("country_code", node_info.country_code.clone()),
            ("continent", node_info.continent_code.clone()),
            ("city", node_info.city.clone()),
            ("isp", node_info.isp.clone()),
            ("os", node_info.operating_system.clone()),
            ("endpoint", endpoint.clone()),
        ]);
        if let Ok(v) = geohash::encode(
            Coord {
                x: node_info.lon,
                y: node_info.lat,
            },
            5,
        ) {
            labels.insert("geohash", v);
        }

        if let Some(result) = response.results.first() {
            if let Some(error) = &result.error {
                // Handle error case
                self.record_failure_with_labels(error, &labels);
            } else if let Some(http_result) = &result.result {
                // Extract status code and other metrics from the HTTP result
                self.record_success_metrics(http_result, result.duration.unwrap_or(0.0), &labels);
            } else {
                error!("Missing http result data");
                return Err(CollectorErrors::MissingData(
                    endpoint.clone(),
                    "http_result",
                ));
            }
        } else {
            error!("No results returned from API");
            return Err(CollectorErrors::MissingData(endpoint.clone(), "no_results"));
        }

        Ok(())
    }
}

impl HttpCollector {
    fn record_failure_with_labels(&self, error: &str, labels: &HashMap<&'static str, String>) {
        let mut labels = labels.clone();
        let error_type = match error {
            e if e.contains("no record found for Query") => "dns_resolution_failed",
            e if e.contains("connection refused") => "connection_refused",
            e if e.contains("connection timed out") => "timeout",
            e if e.contains("name resolution failed") => "resolution_failed",
            e if e.contains("server misbehaving") => "server_misbehaving",
            e if e.contains("network is unreachable") => "network_unreachable",
            e if e.contains("Failed to execute HTTP Request") => "http_request_failed",
            e => {
                warn!(?e, "Unable to parse http error, returning unknown_error");
                "unknown_error"
            }
        };
        labels.insert("error_type", error_type.into());

        counter!(
            format!(
                "{}http_request_error_total",
                self.config.common_config.prefix
            ),
            &labels
        )
        .increment(1);

        counter!(
            format!("{}http_request_total", self.config.common_config.prefix),
            &labels
        )
        .increment(1);
    }

    fn record_success_metrics(
        &self,
        result: &PerformHttpResponseResultsItemResult,
        duration: f64,
        labels: &HashMap<&'static str, String>,
    ) {
        let prefix = &self.config.common_config.prefix;

        // Record request duration
        histogram!(format!("{}http_request_duration_ms", prefix), labels).record(duration);

        // Record status code (statusCode in the API response)
        let mut status_labels = labels.clone();
        status_labels.insert("status_code", result.status_code.to_string());
        gauge!(format!("{}http_status_code", prefix), &status_labels).set(result.status_code);

        // Record body hash (bodyHash in the API response)
        let mut hash_u64 = 0u64;

        // Use first 8 bytes of hash to create a u64
        if result.body_hash.len() >= 16 {
            // Try to parse first 16 chars (8 bytes) of hash as hex
            if let Ok(hash_value) = u64::from_str_radix(&result.body_hash[0..16], 16) {
                hash_u64 = hash_value;
            }
        }

        gauge!(format!("{}http_body_hash", prefix), labels).set(hash_u64 as f64);

        // Record regex matches (matches in the API response)
        let mut match_labels = labels.clone();
        match_labels.insert("match_count", result.matches.len().to_string());

        gauge!(format!("{}http_regex_match_count", prefix), &match_labels)
            .set(result.matches.len() as f64);

        // Record successful request
        counter!(format!("{}http_request_success_total", prefix), labels).increment(1);

        // Record total request
        counter!(format!("{}http_request_total", prefix), labels).increment(1);
    }

    fn hash_records<T: AsRef<str>>(records: &[T]) -> u64 {
        use std::collections::BTreeSet;

        let normalized: BTreeSet<String> =
            records.iter().map(|s| s.as_ref().to_lowercase()).collect();

        let mut hasher = DefaultHasher::new();
        normalized.hash(&mut hasher);
        hasher.finish()
    }
}
