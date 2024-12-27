use super::Collector;
use crate::config::MetricConfig;
use crate::types::{
    PerformHlsBodyContinentCode, PerformHlsBodyCountryCode, PerformHlsBodyMobile,
    PerformHlsBodyProxy, PerformHlsBodyResidential, PerformHlsResponse,
};
use crate::API_CLIENT;
use color_eyre::eyre::Result;
use metrics::{counter, gauge, histogram};
use progenitor::progenitor_client::Error;
use reqwest::StatusCode;
use std::str::FromStr;
use tracing::{error, info, warn};

pub struct HlsCollector {
    config: MetricConfig,
}

impl Collector for HlsCollector {
    fn new(config: MetricConfig) -> Self {
        Self { config }
    }

    fn register_metrics(&self) {
        let prefix = &self.config.prefix;

        // Master playlist metrics
        metrics::describe_histogram!(
            format!("{}hls_master_download_duration_seconds", prefix),
            "Time taken to download master playlist"
        );

        metrics::describe_gauge!(
            format!("{}hls_master_size_bytes", prefix),
            "Size of master playlist in bytes"
        );

        metrics::describe_gauge!(
            format!("{}hls_renditions_count", prefix),
            "Number of available renditions"
        );

        // Connection metrics
        metrics::describe_histogram!(
            format!("{}hls_tcp_connect_duration_seconds", prefix),
            "TCP connection establishment time"
        );

        metrics::describe_histogram!(
            format!("{}hls_ttfb_duration_seconds", prefix),
            "Time to first byte"
        );

        metrics::describe_histogram!(
            format!("{}hls_dns_resolve_duration_seconds", prefix),
            "DNS resolution time"
        );

        metrics::describe_histogram!(
            format!("{}hls_tls_handshake_duration_seconds", prefix),
            "TLS handshake duration"
        );

        // Fragment metrics
        metrics::describe_histogram!(
            format!("{}hls_fragment_download_duration_seconds", prefix),
            "Time taken to download fragments"
        );

        metrics::describe_gauge!(
            format!("{}hls_fragment_size_bytes", prefix),
            "Size of fragments in bytes"
        );

        metrics::describe_gauge!(
            format!("{}hls_fragment_bandwidth_bytes_per_second", prefix),
            "Fragment download bandwidth"
        );

        metrics::describe_gauge!(
            format!("{}hls_fragment_duration_seconds", prefix),
            "Duration of fragments"
        );

        // Rendition metrics
        metrics::describe_gauge!(
            format!("{}hls_rendition_bandwidth_bits_per_second", prefix),
            "Bandwidth of rendition"
        );

        // Success/failure metrics
        metrics::describe_counter!(
            format!("{}hls_failures_total", prefix),
            "Total number of HLS failures"
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
            .and_then(|x| PerformHlsBodyCountryCode::from_str(&x).ok());

        let continent_code = self
            .config
            .network
            .as_ref()
            .and_then(|x| x.continent_code.clone())
            .and_then(|c| PerformHlsBodyContinentCode::from_str(c.as_ref()).ok());

        let mobile = self
            .config
            .network
            .as_ref()
            .map(|n| n.mobile.as_ref().to_uppercase())
            .and_then(|mo| PerformHlsBodyMobile::from_str(&mo).ok())
            .unwrap_or_default();

        let residential = self
            .config
            .network
            .as_ref()
            .map(|n| n.residential.as_ref().to_uppercase())
            .and_then(|mo| PerformHlsBodyResidential::from_str(&mo).ok())
            .unwrap_or_default();

        let proxy = self
            .config
            .network
            .as_ref()
            .map(|n| n.proxy.as_ref().to_uppercase())
            .and_then(|mo| PerformHlsBodyProxy::from_str(&mo).ok())
            .unwrap_or_default();

        info!(?self.config, ?country_code, "Sending DNS request");

        match API_CLIENT
            .perform_hls()
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

impl HlsCollector {
    fn handle_response(&self, response: PerformHlsResponse) {
        if let Some(result) = response.results.first() {
            if let (Some(hls_result), Some(node_info)) = (result.result.clone(), response.node_info)
            {
                let prefix = &self.config.prefix;
                let master = hls_result.master;

                // Record master playlist metrics
                histogram!(
                    format!("{}hls_master_download_duration_seconds", prefix),
                    "country_code" => node_info.country_code.clone(),
                    "continent" => node_info.continent_code.clone(),
                    "city" => node_info.city.clone(),
                    "isp" => node_info.isp.clone(),
                    "endpoint" => result.endpoint.clone()
                )
                .record(result.duration.unwrap_or_default() / 1000.0);

                gauge!(
                    format!("{}hls_master_size_bytes", prefix),
                    "country_code" => node_info.country_code.clone(),
                    "continent" => node_info.continent_code.clone(),
                    "city" => node_info.city.clone(),
                    "isp" => node_info.isp.clone(),
                    "endpoint" => result.endpoint.clone()
                )
                .set(master.clone().unwrap().download_metrics.unwrap().size);

                gauge!(
                    format!("{}hls_renditions_count", prefix),
                    "country_code" => node_info.country_code.clone(),
                    "continent" => node_info.continent_code.clone(),
                    "city" => node_info.city.clone(),
                    "isp" => node_info.isp.clone(),
                    "endpoint" => result.endpoint.clone()
                )
                .set(master.clone().unwrap().renditions.len() as f64);

                // Record master playlist connection metrics
                if let Some(metrics) = master.clone().unwrap().metrics {
                    histogram!(
                        format!("{}hls_tcp_connect_duration_seconds", prefix),
                        "country_code" => node_info.country_code.clone(),
                        "continent" => node_info.continent_code.clone(),
                        "city" => node_info.city.clone(),
                        "isp" => node_info.isp.clone(),
                        "endpoint" => result.endpoint.clone()
                    )
                    .record(metrics.tcp_connect_duration_ms / 1000.0);

                    histogram!(
                        format!("{}hls_ttfb_duration_seconds", prefix),
                        "country_code" => node_info.country_code.clone(),
                        "continent" => node_info.continent_code.clone(),
                        "city" => node_info.city.clone(),
                        "isp" => node_info.isp.clone(),
                        "endpoint" => result.endpoint.clone()
                    )
                    .record(metrics.http_ttfb_duration_ms / 1000.0);

                    // ... other connection metrics
                }

                // Record rendition metrics
                for rendition in master.clone().unwrap().renditions {
                    // Record fragment metrics for each rendition
                    for fragment in rendition.content_fragment_metrics {
                        histogram!(
                            format!("{}hls_fragment_download_duration_seconds", prefix),
                            "country_code" => node_info.country_code.clone(),
                            "continent" => node_info.continent_code.clone(),
                            "city" => node_info.city.clone(),
                            "isp" => node_info.isp.clone(),
                            "endpoint" => result.endpoint.clone(),
                            "resolution" => rendition.resolution.clone(),
                            "bandwidth" => rendition.bandwidth.to_string()
                        )
                        .record(fragment.download_metrics.clone().unwrap().time_ms / 1000.0);

                        gauge!(
                            format!("{}hls_fragment_size_bytes", prefix),
                            "country_code" => node_info.country_code.clone(),
                            "continent" => node_info.continent_code.clone(),
                            "city" => node_info.city.clone(),
                            "isp" => node_info.isp.clone(),
                            "endpoint" => result.endpoint.clone(),
                            "resolution" => rendition.resolution.clone(),
                            "bandwidth" => rendition.bandwidth.to_string()
                        )
                        .set(fragment.download_metrics.clone().unwrap().size);

                        gauge!(
                            format!("{}hls_fragment_bandwidth_bytes_per_second", prefix),
                            "country_code" => node_info.country_code.clone(),
                            "continent" => node_info.continent_code.clone(),
                            "city" => node_info.city.clone(),
                            "isp" => node_info.isp.clone(),
                            "endpoint" => result.endpoint.clone(),
                            "resolution" => rendition.resolution.clone(),
                            "bandwidth" => rendition.bandwidth.to_string()
                        )
                        .set(fragment.download_metrics.clone().unwrap().bytes_per_second);
                    }
                }
            } else {
                error!("Missing HLS result or node info");
                self.record_failure("missing_data");
            }
        } else {
            error!("No results returned from API");
            self.record_failure("no_results");
        }
    }

    fn handle_error(&self, e: Error<PerformHlsResponse>) {
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
            format!("{}hls_failures_total", self.config.prefix),
            "error_type" => error_type.to_string()
        )
        .increment(1);
    }
}
