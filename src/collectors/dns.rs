use super::Collector;
use crate::config::MetricConfig;
use crate::types::{
    PerformDnsBodyContinentCode, PerformDnsBodyCountryCode, PerformDnsBodyMobile,
    PerformDnsBodyProxy, PerformDnsBodyResidential, PerformDnsResponse,
};
use crate::API_CLIENT;
use color_eyre::eyre::Result;
use metrics::{counter, gauge, histogram};
use progenitor::progenitor_client::Error;
use reqwest::StatusCode;
use std::str::FromStr;
use tracing::{error, info, warn};

pub struct DnsCollector {
    config: MetricConfig,
}

impl Collector for DnsCollector {
    fn new(config: MetricConfig) -> Self {
        Self { config }
    }

    fn register_metrics(&self) {
        let prefix = &self.config.prefix;

        metrics::describe_counter!(
            format!("{}dns_lookup_failures_total", prefix),
            "Total number of DNS lookup failures"
        );

        metrics::describe_histogram!(
            format!("{}dns_lookup_duration_seconds", prefix),
            "Time taken to perform DNS lookup in seconds"
        );

        metrics::describe_gauge!(
            format!("{}dns_records_returned", prefix),
            "Number of DNS records returned by type"
        );

        metrics::describe_gauge!(
            format!("{}dns_lookup_success_ratio", prefix),
            "Ratio of successful DNS lookups"
        );
    }

    fn get_config(&self) -> &MetricConfig {
        &self.config
    }

    async fn perform_request(&self) -> Result<()> {
        let country_code = self
            .config
            .network
            .as_ref()
            .and_then(|x| x.country_code)
            .map(|c| c.to_alpha2().to_string())
            .and_then(|x| PerformDnsBodyCountryCode::from_str(&x).ok());

        let continent_code = self
            .config
            .network
            .as_ref()
            .and_then(|x| x.continent_code.clone())
            .and_then(|c| PerformDnsBodyContinentCode::from_str(c.as_ref()).ok());

        let mobile = self
            .config
            .network
            .as_ref()
            .map(|n| n.mobile.as_ref().to_uppercase())
            .and_then(|mo| PerformDnsBodyMobile::from_str(&mo).ok())
            .unwrap_or_default();

        let residential = self
            .config
            .network
            .as_ref()
            .map(|n| n.residential.as_ref().to_uppercase())
            .and_then(|mo| PerformDnsBodyResidential::from_str(&mo).ok())
            .unwrap_or_default();

        let proxy = self
            .config
            .network
            .as_ref()
            .map(|n| n.proxy.as_ref().to_uppercase())
            .and_then(|mo| PerformDnsBodyProxy::from_str(&mo).ok())
            .unwrap_or_default();

        info!(?self.config, ?country_code, "Sending DNS request");

        match API_CLIENT
            .perform_dns()
            .body_map(|body| {
                body.hostnames([self.config.endpoint.clone()])
                    .country_code(country_code)
                    .continent_code(continent_code)
                    .mobile(mobile)
                    .residential(residential)
                    .proxy(proxy)
            })
            .send()
            .await
        {
            Ok(response) => {
                let response = response.into_inner();
                self.handle_response(response);
                Ok(())
            }
            Err(e) => {
                self.handle_error(e);
                Ok(())
            }
        }
    }
}

impl DnsCollector {
    fn handle_response(&self, response: PerformDnsResponse) {
        if let Some(result) = response.results.first() {
            if let (Some(dns_result), Some(node_info)) = (result.result.clone(), response.node_info)
            {
                let prefix = &self.config.prefix;

                // Record lookup duration
                histogram!(
                    format!("{}dns_lookup_duration_seconds", prefix),
                    "country_code" => node_info.country_code.clone(),
                    "continent" => node_info.continent_code.clone(),
                    "city" => node_info.city.clone(),
                    "isp" => node_info.isp.clone(),
                    "endpoint" => result.endpoint.clone()
                )
                .record(result.duration.unwrap_or(0.0));

                // Record success ratio
                gauge!(
                    format!("{}dns_lookup_success_ratio", prefix),
                    "country_code" => node_info.country_code.clone(),
                    "continent" => node_info.continent_code.clone(),
                    "city" => node_info.city.clone(),
                    "isp" => node_info.isp.clone(),
                    "endpoint" => result.endpoint.clone()
                )
                .set(1.0);

                // Record number of returned records
                gauge!(
                    format!("{}dns_records_returned", prefix),
                    "country_code" => node_info.country_code.clone(),
                    "continent" => node_info.continent_code.clone(),
                    "city" => node_info.city.clone(),
                    "isp" => node_info.isp.clone(),
                    "record_type" => "IP",
                    "endpoint" => result.endpoint.clone()
                )
                .set(dns_result.ips.len() as f64);

                // Increment success counter
                counter!(
                    format!("{}dns_lookup_success_total", prefix),
                    "country_code" => node_info.country_code.clone(),
                    "continent" => node_info.continent_code.clone(),
                    "city" => node_info.city.clone(),
                    "isp" => node_info.isp.clone(),
                    "endpoint" => result.endpoint.clone()
                )
                .increment(1);
            } else {
                error!("Missing DNS result or node info");
                self.record_failure("missing_data");
            }
        } else {
            error!("No results returned from API");
            self.record_failure("no_results");
        }
    }

    fn handle_error(&self, e: Error<PerformDnsResponse>) {
        let error_type = if let Some(StatusCode::NOT_FOUND) = e.status() {
            "no_nodes_found"
        } else {
            "api_error"
        };

        self.record_failure(error_type);

        if let Some(StatusCode::NOT_FOUND) = e.status() {
            warn!(?self.config.network, "No nodes were found for the given criteria");
        } else {
            error!("API request failed: {:#?}", e);
        }
    }

    fn record_failure(&self, error_type: &str) {
        counter!(
            format!("{}dns_lookup_failures_total", self.config.prefix),
            "error_type" => error_type.to_string()
        )
        .increment(1);
    }
}
