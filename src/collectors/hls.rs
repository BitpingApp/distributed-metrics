use super::{Collector, CollectorErrors};
use crate::config::{HlsConfig, MetricConfig};
use crate::types::*;
use crate::API_CLIENT;
use color_eyre::eyre::Result;
use metrics::{counter, gauge, histogram};
use progenitor::progenitor_client::Error;
use reqwest::StatusCode;
use std::{collections::HashMap, str::FromStr};
use tracing::{debug, error, info, warn};

pub struct HlsCollector {
    config: &'static HlsConfig,
}

type NodeInfo = PerformHlsResponseNodeInfo;
type HlsMaster = PerformHlsResponseResultsItemResultMaster;

#[derive(Debug)]
struct MetricLabels {
    country_code: String,
    continent: String,
    city: String,
    isp: String,
    endpoint: String,
}

impl Collector for HlsCollector {
    type Config = HlsConfig;
    type Response = PerformHlsResponse;

    fn new(config: &'static HlsConfig) -> Self {
        Self { config }
    }

    fn register_metrics(&self) {
        let prefix = &self.config.common_config.prefix;

        metrics::describe_histogram!(
            format!("{}hls_total_ms", prefix),
            "Total time taken to perform HLS test"
        );


        // Master playlist metrics
        metrics::describe_histogram!(
            format!("{}hls_master_download_ms", prefix),
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
        metrics::describe_gauge!(
            format!("{}hls_master_bitrate", prefix),
            "Master playlist download bits per second"
        );
        // Master Connection metrics
        metrics::describe_histogram!(
            format!("{}hls_master_tcp_connect_ms", prefix),
            "Master Manifest TCP connection establishment time"
        );
        metrics::describe_histogram!(
            format!("{}hls_master_http_get_send_ms", prefix),
            "Master Manifest HTTP GET request send duration"
        );

        metrics::describe_histogram!(format!("{}hls_master_ttfb_ms", prefix), "Master Manifest Time to first byte");
        metrics::describe_histogram!(
            format!("{}hls_master_dns_resolve_ms", prefix),
            "Master Manifest DNS resolution time"
        );
        metrics::describe_histogram!(
            format!("{}hls_master_tls_handshake_ms", prefix),
            "Master Manifest TLS handshake duration"
        );

        // Fragment metrics
        metrics::describe_histogram!(
            format!("{}hls_fragment_download_ms", prefix),
            "Fragment download time"
        );
        metrics::describe_gauge!(
            format!("{}hls_fragment_size_bytes", prefix),
            "Fragment size"
        );
        metrics::describe_gauge!(
            format!("{}hls_fragment_bandwidth_bytes_per", prefix),
            "Fragment bandwidth"
        );
        metrics::describe_gauge!(
            format!("{}hls_fragment_duration_seconds", prefix),
            "Fragment duration in seconds"
        );
        metrics::describe_gauge!(
            format!("{}hls_fragment_sequence_discontinuity", prefix),
            "Fragment sequence discontinuities"
        );

        // Quality metrics
        metrics::describe_gauge!(
            format!("{}hls_buffer_fill_rate", prefix),
            "Rate at which buffer is filling relative to playback speed (>1 means faster than realtime)"
        );

        metrics::describe_gauge!(
            format!("{}hls_estimated_buffer_ms", prefix),
            "Estimated buffer length in milliseconds based on download speeds"
        );

        metrics::describe_histogram!(
            format!("{}hls_initial_buffer_ms", prefix),
            "Time taken to load initial buffer including master playlist, variant playlist, and first segment"
        );

        metrics::describe_histogram!(
            format!("{}hls_playlist_chain_load_time", prefix),
            "Time taken to load master and variant playlists"
        );


        // Error metrics
        metrics::describe_counter!(
            format!("{}hls_failures_total", prefix),
            "Total number of failures"
        );
        metrics::describe_counter!(
            format!("{}hls_errors_by_type", prefix),
            "Errors categorized by type"
        );
    }

    fn get_frequency(&self) -> std::time::Duration {
        self.config.common_config.frequency
    }

    async fn perform_request(&self) -> Result<Self::Response> {
        let network_config = self.config.common_config.network.as_ref();

        let country_code = network_config
            .and_then(|x| x.country_code)
            .map(|c| c.to_alpha2().to_string())
            .and_then(|x| PerformHlsBodyCountryCode::from_str(&x).ok());

        let continent_code = network_config
            .and_then(|x| x.continent_code.clone())
            .and_then(|c| PerformHlsBodyContinentCode::from_str(c.as_ref()).ok());

        let mobile = network_config
            .map(|n| n.mobile.as_ref().to_uppercase())
            .and_then(|mo| PerformHlsBodyMobile::from_str(&mo).ok())
            .unwrap_or_default();

        let residential = network_config
            .map(|n| n.residential.as_ref().to_uppercase())
            .and_then(|mo| PerformHlsBodyResidential::from_str(&mo).ok())
            .unwrap_or_default();

        let proxy = network_config
            .map(|n| n.proxy.as_ref().to_uppercase())
            .and_then(|mo| PerformHlsBodyProxy::from_str(&mo).ok())
            .unwrap_or_default();

        let response = API_CLIENT
            .perform_hls()
            .body_map(|body| {
                body.hostnames([self.config.common_config.endpoint.clone()])
                    .country_code(country_code)
                    .continent_code(continent_code)
                    .mobile(mobile)
                    .residential(residential)
                    .proxy(proxy)
                    .configuration(PerformHlsBodyConfiguration {
                        headers: self.config.headers.clone(),
                    })
            })
            .send()
            .await
            .inspect_err(|e| {
                self.handle_error(e);
            })?;

        Ok(response.into_inner())
    }

    fn handle_response(&self, response: PerformHlsResponse) -> Result<(), CollectorErrors> {
        let prefix = &self.config.common_config.prefix;
        let endpoint = self.config.common_config.endpoint.clone();

        let node_info = response
            .node_info
            .ok_or_else(|| CollectorErrors::MissingNodeInfo(endpoint.clone()))?;

        let labels = [
            ("country_code", node_info.country_code.clone()),
            ("continent", node_info.continent_code.clone()),
            ("city", node_info.city.clone()),
            ("isp", node_info.isp.clone()),
            ("os", node_info.operating_system.clone()),
            ("endpoint", endpoint.clone()),
        ];

        match response.results.first() {
            Some(result) => {
                if let Some(hls_result) = &result.result {
                    histogram!(format!("{}hls_total_ms", self.config.common_config.prefix), &labels).record(result.duration.unwrap_or_default());

                    if let Some(master) = &hls_result.master {
                        self.record_master_metrics(&labels, master)?;
                        for rendition in &master.renditions {
                            self.record_rendition_metrics(&labels, Some(master), &rendition.clone().into())?;
                        }
                    }

                    if let Some(rendition) = &hls_result.rendition {
                        self.record_rendition_metrics(&labels, None, rendition)?;
                    }

                    Ok(())
                } else {
                    self.record_failure("missing_result");
                    Err(CollectorErrors::MissingData(endpoint, "hls_result"))
                }
            }
            _ => {
                self.record_failure("missing_data");
                Err(CollectorErrors::MissingData(endpoint, "hls"))
            }
        }
    }
}

impl HlsCollector {
    fn record_master_metrics(
        &self,
        labels: &[(&'static str, String)],
        master: &PerformHlsResponseResultsItemResultMaster
    ) -> Result<(), CollectorErrors> {
        let prefix = &self.config.common_config.prefix;

        // Master Manifest TTFB Metrics
        if let Some(metrics) = &master.metrics {
            histogram!(format!("{}hls_master_tcp_connect_ms", prefix), labels)
            .record(metrics.tcp_connect_duration_ms);

            histogram!(format!("{}hls_master_ttfb_ms", prefix), labels)
                .record(metrics.http_ttfb_duration_ms);

            histogram!(format!("{}hls_master_dns_resolve_ms", prefix), labels)
                .record(metrics.dns_resolve_duration_ms.unwrap_or_default());

            histogram!(format!("{}hls_master_tls_handshake_ms", prefix), labels)
                .record(metrics.tls_handshake_duration_ms.unwrap_or_default());
        }
        
        if let Some(download_metrics) = &master.download_metrics {
            gauge!(format!("{}hls_master_size_bytes", prefix), labels).set(download_metrics.size);

            histogram!(format!("{}hls_master_download_ms", prefix), labels)
                .record(download_metrics.time_ms);

            gauge!(format!("{}hls_master_bitrate", prefix), labels).set(download_metrics.bytes_per_second * 8.0);
        }

        gauge!(format!("{}hls_renditions_count", prefix), labels).set(master.renditions.len() as f64);
        Ok(())
    }

    /// Useful Metrics:
    /// - Did the Client download the fragment in the target duration time?
    /// - What was the "Ratio" percentage they actually downloaded at (how many percent faster or
    /// slower were they than the target?)
    /// -
    fn record_rendition_metrics(
        &self,
        labels: &[(&'static str, String)],
        master: Option<&PerformHlsResponseResultsItemResultMaster>,        
        rendition: &PerformHlsResponseResultsItemResultRendition,
    ) -> Result<(), CollectorErrors> {
        let prefix = &self.config.common_config.prefix;

        let mut labels = labels.to_vec();
        labels.push(("resolution", rendition.resolution.clone()));
        labels.push(("bandwidth", rendition.bandwidth.to_string()));
        labels.push(("target_duration_secs", rendition.target_duration_secs.to_string()));
        labels.push(("discontinuity_sequence", rendition.discontinuity_sequence.to_string()));
        labels.push(("playlist_type", if master.is_some() { "variant" } else { "direct" }.to_string()));
        let labels = &labels;

        for fragment in &rendition.content_fragment_metrics {
            gauge!(
                format!("{}hls_fragment_download_ratio", prefix),
                labels
            ).set(fragment.download_ratio);
        

            if let Some(metrics) = &fragment.download_metrics {
                histogram!(format!("{}hls_fragment_download_ms", prefix), labels)
                    .record(metrics.time_ms);

                gauge!(format!("{}hls_fragment_size_bytes", prefix), labels).set(metrics.size);

                gauge!(
                    format!("{}hls_fragment_bandwidth_bytes_per_second", prefix),
                    labels
                )
                .set(metrics.bytes_per_second);
            }

            if let Some(metrics) = &fragment.metrics {
                histogram!(format!("{}hls_fragment_tcp_connect_ms", prefix), labels)
                .record(metrics.tcp_connect_duration_ms);

                histogram!(format!("{}hls_fragment_ttfb_ms", prefix), labels)
                    .record(metrics.http_ttfb_duration_ms);

                histogram!(format!("{}hls_fragment_dns_resolve_ms", prefix), labels)
                    .record(metrics.dns_resolve_duration_ms.unwrap_or_default());

                histogram!(format!("{}hls_fragment_tls_handshake_ms", prefix), labels)
                    .record(metrics.tls_handshake_duration_ms.unwrap_or_default());
            }

            gauge!(format!("{}hls_fragment_duration_seconds", prefix), labels)
                .set(fragment.content_fragment_duration_secs);

        }

        self.calculate_buffer_metrics(labels, master, rendition)?;


        Ok(())
    }

    
    fn calculate_buffer_metrics(
        &self,
        labels: &[(&'static str, String)],
        master: Option<&PerformHlsResponseResultsItemResultMaster>,
        rendition: &PerformHlsResponseResultsItemResultRendition,
    ) -> Result<(), CollectorErrors> {
        let prefix = &self.config.common_config.prefix;

        // Safely calculate playlist chain load time
        let master_load_time = master
            .and_then(|m| m.metrics.as_ref())
            .map(|m| {
                m.dns_resolve_duration_ms.unwrap_or(0.0) as f64 +
                m.tls_handshake_duration_ms.unwrap_or(0.0) as f64 +
                m.tcp_connect_duration_ms as f64 +
                m.http_ttfb_duration_ms as f64
            })
            .unwrap_or(0.0);

        let variant_load_time = rendition.metrics.as_ref()
            .map(|m| {
                m.dns_resolve_duration_ms.unwrap_or(0.0) as f64 +
                m.tls_handshake_duration_ms.unwrap_or(0.0) as f64 +
                m.tcp_connect_duration_ms as f64 +
                m.http_ttfb_duration_ms as f64
            })
            .unwrap_or(0.0);

        let playlist_chain_load_time = master_load_time + variant_load_time;

        // Record playlist chain load time safely
        if playlist_chain_load_time >= 0.0 {
            histogram!(
                format!("{}hls_playlist_chain_load_time", prefix),
                labels
            ).record(playlist_chain_load_time);
        } else {
            warn!("Invalid playlist chain load time: {}", playlist_chain_load_time);
        }

        // Safely handle first fragment calculations
        if let Some(first_fragment) = rendition.content_fragment_metrics.first() {
            // Calculate first segment load time safely
            let first_segment_load_time = first_fragment.metrics.as_ref()
                .map(|m| {
                    m.dns_resolve_duration_ms.unwrap_or(0.0) as f64 +
                    m.tls_handshake_duration_ms.unwrap_or(0.0) as f64 +
                    m.tcp_connect_duration_ms as f64 +
                    m.http_ttfb_duration_ms as f64 +
                    first_fragment.download_metrics
                        .as_ref()
                        .map(|dm| dm.time_ms as f64)
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0);

            let initial_buffer_duration = playlist_chain_load_time + first_segment_load_time;

            // Record initial buffer duration safely
            if initial_buffer_duration >= 0.0 {
                histogram!(
                    format!("{}hls_initial_buffer_ms", prefix),
                    labels
                ).record(initial_buffer_duration);
            } else {
                warn!("Invalid initial buffer duration: {}", initial_buffer_duration);
            }

            // Record buffer fill rate safely
            if first_fragment.download_ratio.is_finite() && first_fragment.download_ratio > 0.0 {
                gauge!(
                    format!("{}hls_buffer_fill_rate", prefix),
                    labels
                ).set(first_fragment.download_ratio);
            } else {
                warn!("Invalid download ratio: {}", first_fragment.download_ratio);
                counter!(
                    format!("{}hls_buffer_calculation_errors", prefix),
                    "error_type" => "invalid_download_ratio"
                ).increment(1);
            }

            // Calculate and record estimated buffer duration safely
            if first_fragment.download_ratio.is_finite() && 
            first_fragment.download_ratio > 1.0 && 
            first_fragment.content_fragment_duration_secs > 0.0 {
                let estimated_buffer = (first_fragment.download_ratio - 1.0) * 
                                        first_fragment.content_fragment_duration_secs * 1000.0;
                
                if estimated_buffer.is_finite() && estimated_buffer >= 0.0 {
                    gauge!(
                        format!("{}hls_estimated_buffer_ms", prefix),
                        labels
                    ).set(estimated_buffer);
                } else {
                    warn!("Invalid estimated buffer duration: {}", estimated_buffer);
                    counter!(
                        format!("{}hls_buffer_calculation_errors", prefix),
                        "error_type" => "invalid_buffer_ms"
                    ).increment(1);
                }
            } else {
                warn!(
                    "Invalid values for buffer calculation: ratio={}, duration={}",
                    first_fragment.download_ratio,
                    first_fragment.content_fragment_duration_secs
                );
                counter!(
                    format!("{}hls_buffer_calculation_errors", prefix),
                    "error_type" => "invalid_calculation_parameters"
                ).increment(1);
            }
        } else {
            warn!("No fragments available for buffer calculations");
            counter!(
                format!("{}hls_buffer_calculation_errors", prefix),
                "error_type" => "no_fragments"
            ).increment(1);
        }

        Ok(())
    }



    fn handle_error(&self, error: &Error<PerformHlsResponse>) {
        let error_type = match error.status() {
            Some(StatusCode::NOT_FOUND) => {
                warn!(?self.config.common_config.network, "No nodes found for criteria");
                "no_nodes_found"
            }
            Some(StatusCode::UNAUTHORIZED) => "unauthorized",
            Some(StatusCode::TOO_MANY_REQUESTS) => "rate_limited",
            _ => {
                error!("API request failed: {:#?}", error);
                "api_error"
            }
        };

        self.record_failure(error_type);
    }

    fn record_failure(&self, error_type: &str) {
        counter!(
            format!("{}hls_failures_total", self.config.common_config.prefix),
            "error_type" => error_type.to_string()
        )
        .increment(1);
    }
}

impl Into<PerformHlsResponseResultsItemResultRendition>
    for PerformHlsResponseResultsItemResultMasterRenditionsItem
{
    fn into(self) -> PerformHlsResponseResultsItemResultRendition {
        PerformHlsResponseResultsItemResultRendition {
            bandwidth: self.bandwidth,
            content_fragment_metrics: self
                .content_fragment_metrics
                .iter()
                .map(
                    |cfm| PerformHlsResponseResultsItemResultRenditionContentFragmentMetricsItem {
                        content_fragment_duration_secs: cfm.content_fragment_duration_secs,
                        download_metrics: cfm.download_metrics.as_ref().map(|dm| PerformHlsResponseResultsItemResultRenditionContentFragmentMetricsItemDownloadMetrics { 
                            bytes_per_second: dm.bytes_per_second,
                            size: dm.size,
                            time_ms: dm.time_ms
                        }),
                        download_ratio: cfm.download_ratio,
                        file: cfm.file.clone(),
                        metrics: cfm.metrics.as_ref().map(|m| PerformHlsResponseResultsItemResultRenditionContentFragmentMetricsItemMetrics{
                            dns_resolve_duration_ms: m.dns_resolve_duration_ms,
                            http_get_send_duration_ms: m.http_get_send_duration_ms,
                            http_ttfb_duration_ms: m.http_ttfb_duration_ms,
                            tcp_connect_duration_ms: m.tcp_connect_duration_ms,
                            tls_handshake_duration_ms: m.tls_handshake_duration_ms
                        }),
                    },
                )
                .collect(),
            discontinuity_sequence: self.discontinuity_sequence,
            download_metrics: self.download_metrics.map(|dm| {
                PerformHlsResponseResultsItemResultRenditionDownloadMetrics {
                    bytes_per_second: dm.bytes_per_second,
                    size: dm.size,
                    time_ms: dm.time_ms,
                }
            }),
            file: self.file,
            metrics: self
                .metrics
                .map(|m| PerformHlsResponseResultsItemResultRenditionMetrics {
                    dns_resolve_duration_ms: m.dns_resolve_duration_ms,
                    http_get_send_duration_ms: m.http_get_send_duration_ms,
                    http_ttfb_duration_ms: m.http_ttfb_duration_ms,
                    tcp_connect_duration_ms: m.tcp_connect_duration_ms,
                    tls_handshake_duration_ms: m.tls_handshake_duration_ms,
                }),
            resolution: self.resolution,
            target_duration_secs: self.target_duration_secs,
        }
    }
}
