use super::{Collector, CollectorErrors};
use crate::config::{IcmpConfig, MetricConfig};
use crate::types::{
    PerformIcmpBodyContinentCode, PerformIcmpBodyCountryCode, PerformIcmpBodyMobile,
    PerformIcmpBodyProxy, PerformIcmpBodyResidential, PerformIcmpResponse,
};
use crate::API_CLIENT;
use color_eyre::eyre::Result;
use metrics::{counter, gauge, histogram};
use progenitor::progenitor_client::Error;
use reqwest::StatusCode;
use std::str::FromStr;
use tracing::{error, info, warn};

pub struct IcmpCollector {
    config: &'static IcmpConfig,
}

impl Collector for IcmpCollector {
    type Config = IcmpConfig;
    type Response = PerformIcmpResponse;

    fn new(config: &'static IcmpConfig) -> Self {
        Self { config }
    }

    fn register_metrics(&self) {
        let prefix = &self.config.common_config.prefix;

        // Basic counters
        metrics::describe_counter!(
            format!("{}icmp_ping_failures_total", prefix),
            "Total number of ICMP ping failures"
        );

        metrics::describe_counter!(
            format!("{}icmp_ping_success_total", prefix),
            "Total number of successful ICMP pings"
        );

        // Latency metrics
        metrics::describe_histogram!(
            format!("{}icmp_ping_duration_seconds", prefix),
            "Time taken to perform ICMP ping in seconds"
        );

        metrics::describe_gauge!(
            format!("{}icmp_ping_latency_min_ms", prefix),
            "Minimum latency of ICMP ping in milliseconds"
        );

        metrics::describe_gauge!(
            format!("{}icmp_ping_latency_max_ms", prefix),
            "Maximum latency of ICMP ping in milliseconds"
        );

        metrics::describe_gauge!(
            format!("{}icmp_ping_latency_avg_ms", prefix),
            "Average latency of ICMP ping in milliseconds"
        );

        metrics::describe_gauge!(
            format!("{}icmp_ping_latency_stddev_ms", prefix),
            "Standard deviation of ICMP ping latency in milliseconds"
        );

        // Packet metrics
        metrics::describe_gauge!(
            format!("{}icmp_ping_packet_loss_ratio", prefix),
            "Ratio of lost packets during ICMP ping"
        );

        metrics::describe_gauge!(
            format!("{}icmp_ping_packets_sent", prefix),
            "Number of ICMP packets sent"
        );

        metrics::describe_gauge!(
            format!("{}icmp_ping_packets_received", prefix),
            "Number of ICMP packets received"
        );

        metrics::describe_gauge!(
            format!("{}icmp_ping_success_ratio", prefix),
            "Ratio of successful ICMP pings"
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
            .and_then(|x| PerformIcmpBodyCountryCode::from_str(&x).ok());

        let continent_code = self
            .config
            .common_config
            .network
            .as_ref()
            .and_then(|x| x.continent_code.clone())
            .and_then(|c| PerformIcmpBodyContinentCode::from_str(c.as_ref()).ok());

        let mobile = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.mobile.as_ref().to_uppercase())
            .and_then(|mo| PerformIcmpBodyMobile::from_str(&mo).ok())
            .unwrap_or_default();

        let residential = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.residential.as_ref().to_uppercase())
            .and_then(|mo| PerformIcmpBodyResidential::from_str(&mo).ok())
            .unwrap_or_default();

        let proxy = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.proxy.as_ref().to_uppercase())
            .and_then(|mo| PerformIcmpBodyProxy::from_str(&mo).ok())
            .unwrap_or_default();

        info!(?self.config.common_config, ?country_code, "Sending ICMP request");

        let response = API_CLIENT
            .perform_icmp()
            .body_map(|body| {
                body.hostnames([self.config.common_config.endpoint.clone()])
                    .country_code(country_code)
                    .continent_code(continent_code)
                    .mobile(mobile)
                    .residential(residential)
                    .proxy(proxy)
            })
            .send()
            .await?;

        Ok(response.into_inner())
    }

    fn get_frequency(&self) -> std::time::Duration {
        self.config.common_config.frequency
    }

    fn handle_response(&self, response: PerformIcmpResponse) -> Result<(), CollectorErrors> {
        if let Some(result) = response.results.first() {
            if let (Some(icmp_result), Some(node_info)) =
                (result.result.clone(), response.node_info)
            {
                let prefix = &self.config.common_config.prefix;

                let labels = [
                    ("country_code", node_info.country_code.clone()),
                    ("continent", node_info.continent_code.clone()),
                    ("city", node_info.city.clone()),
                    ("isp", node_info.isp.clone()),
                    ("os", node_info.operating_system.clone()),
                    // ("residential", node_info.residential.to_string()),
                    // ("proxy", node_info.proxy.to_string()),
                    // ("mobile", node_info.mobile.to_string()),
                    ("endpoint", result.endpoint.clone()),
                    ("ip_address", icmp_result.ip_address.clone()),
                ];

                // Record overall request duration
                histogram!(format!("{}icmp_ping_duration_seconds", prefix), &labels)
                    .record(result.duration.unwrap_or(0.0) / 1000.0); // Convert ms to seconds

                // Record latency metrics
                gauge!(format!("{}icmp_ping_latency_min_ms", prefix), &labels).set(icmp_result.min);

                gauge!(format!("{}icmp_ping_latency_max_ms", prefix), &labels).set(icmp_result.max);

                gauge!(format!("{}icmp_ping_latency_avg_ms", prefix), &labels).set(icmp_result.avg);

                gauge!(format!("{}icmp_ping_latency_stddev_ms", prefix), &labels)
                    .set(icmp_result.std_dev);

                // Record packet metrics
                gauge!(format!("{}icmp_ping_packet_loss_ratio", prefix), &labels)
                    .set(icmp_result.packet_loss / 100.0);

                gauge!(format!("{}icmp_ping_packets_sent", prefix), &labels)
                    .set(icmp_result.packets_sent);

                gauge!(format!("{}icmp_ping_packets_received", prefix), &labels)
                    .set(icmp_result.packets_recv);

                // Record success metrics
                let success_ratio = if icmp_result.packets_sent > 0.0 {
                    icmp_result.packets_recv / icmp_result.packets_sent
                } else {
                    0.0
                };

                gauge!(format!("{}icmp_ping_success_ratio", prefix), &labels).set(success_ratio);

                counter!(format!("{}icmp_ping_success_total", prefix), &labels).increment(1);
            } else {
                error!("Missing ICMP result or node info");
                self.record_failure("missing_data");
            }
        } else {
            error!("No results returned from API");
            self.record_failure("no_results");
        }

        Ok(())
    }
}

impl IcmpCollector {
    fn handle_error(&self, e: Error<PerformIcmpResponse>) {
        let error_type = if let Some(StatusCode::NOT_FOUND) = e.status() {
            "no_nodes_found"
        } else {
            "api_error"
        };

        self.record_failure(error_type);

        if let Some(StatusCode::NOT_FOUND) = e.status() {
            warn!(?self.config.common_config.network, "No nodes were found for the given criteria");
        } else {
            error!("API request failed: {:#?}", e);
        }
    }

    fn record_failure(&self, error_type: &str) {
        counter!(
            format!("{}icmp_ping_failures_total", self.config.common_config.prefix),
            "error_type" => error_type.to_string()
        )
        .increment(1);
    }
}
