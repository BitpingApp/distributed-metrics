use super::cdn_headers::{
    parse_cache_age_seconds, parse_cache_status, parse_cdn_provider, parse_edge_pop, CdnProvider,
};
use super::{Collector, CollectorErrors};
use crate::types::{
    PerformHttpBodyConfiguration, PerformHttpBodyContinentCode, PerformHttpBodyCountryCode,
    PerformHttpBodyMobile, PerformHttpBodyProxy, PerformHttpBodyResidential, PerformHttpResponse,
    PerformHttpResponseResultsItemResult,
};
use crate::API_CLIENT;
use color_eyre::eyre::Result;
use distributed_metrics::config::HttpConfig;
use geohash::Coord;
use metrics::{counter, gauge, histogram};
use std::collections::HashMap;
use std::str::FromStr;
use chrono::{DateTime, Utc};
use tracing::{error, info, warn};
use xxhash_rust::xxh3::xxh3_64;

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

        metrics::describe_gauge!(
            format!("{}http_status_match", prefix),
            "Whether the HTTP status code matched the expected status codes (1=match, 0=mismatch). Only emitted when status_codes is configured."
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

        metrics::describe_histogram!(
            format!("{}http_dns_resolve_ms", prefix),
            "DNS resolution phase duration (ms). Skipped when the address was cached."
        );

        metrics::describe_histogram!(
            format!("{}http_tcp_connect_ms", prefix),
            "TCP connect phase duration (ms). Skipped on QUIC/HTTP3."
        );

        metrics::describe_histogram!(
            format!("{}http_tls_handshake_ms", prefix),
            "TLS handshake phase duration (ms). Skipped on plaintext HTTP and QUIC."
        );

        metrics::describe_histogram!(
            format!("{}http_ttfb_ms", prefix),
            "HTTP time-to-first-byte from request send to first response byte (ms)."
        );

        metrics::describe_histogram!(
            format!("{}http_content_download_ms", prefix),
            "HTTP response body download phase duration (ms)."
        );

        metrics::describe_gauge!(
            format!("{}http_protocol", prefix),
            "Negotiated HTTP protocol version (h1|h2|h3). Set to 1 for the active series."
        );

        metrics::describe_gauge!(
            format!("{}http_address_family", prefix),
            "IP address family used for the request (ipv4|ipv6). Set to 1 for the active series."
        );

        metrics::describe_counter!(
            format!("{}http_fallback_total", prefix),
            "Protocol fallbacks observed in the fallback chain (e.g. h3→h2 on AUTO transport)."
        );

        metrics::describe_gauge!(
            format!("{}http_cdn_provider", prefix),
            "Detected CDN provider from response headers. Set to 1 for the active series."
        );

        metrics::describe_gauge!(
            format!("{}http_cache_status", prefix),
            "Cache result reported by the CDN (hit|miss|stale|expired|bypass|dynamic|unknown). Set to 1 for the active series."
        );

        metrics::describe_gauge!(
            format!("{}http_edge_pop", prefix),
            "CDN edge POP code that served the response. Set to 1 for the active series."
        );

        metrics::describe_gauge!(
            format!("{}http_cache_age_seconds", prefix),
            "Value of the response Age header, in seconds."
        );

        metrics::describe_gauge!(
            format!("{}http_ssl_expires_days_remaining", prefix),
            "Days remaining until the served TLS certificate expires."
        );

        metrics::describe_gauge!(
            format!("{}http_ssl_chain_valid", prefix),
            "Whether the served TLS certificate chain validates against the system trust store (1=valid, 0=invalid)."
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
                        // New optional probe options (BIT-562). None = server
                        // defaults (no SSL-cert fetch, TCP transport).
                        ssl_info: None,
                        transport: Default::default(),
                        status_codes: self
                            .config
                            .status_codes
                            .as_ref()
                            .map(|codes| codes.iter().map(|&c| c as f64).collect())
                            .unwrap_or_default(),
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
        let endpoint = &self.config.common_config.endpoint;

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
        if let Some(name) = &self.config.common_config.name {
            labels.insert("endpoint_name", name.clone());
        }
        if let Ok(v) = geohash::encode(
            Coord {
                x: node_info.lon,
                y: node_info.lat,
            },
            5,
        ) {
            labels.insert("geohash", v);
        }
        self.config.common_config.filter_labels(&mut labels);

        let prefix = &self.config.common_config.prefix;
        counter!(format!("{}http_request_success_total", prefix), &labels).increment(0);
        counter!(format!("{}http_request_total", prefix), &labels).increment(0);

        if let Some(result) = response.results.first() {
            if let Some(error) = &result.error {
                // Handle error case — pass the proto's typed `error_code` as the
                // ground-truth classification (the free `error` string only
                // survives for warn-logging the missing/unknown-code path).
                self.record_failure_with_labels(
                    error,
                    result.error_code.as_deref(),
                    &labels,
                );
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
    fn record_failure_with_labels(
        &self,
        error: &str,
        error_code: Option<&str>,
        labels: &HashMap<&'static str, String>,
    ) {
        let prefix = &self.config.common_config.prefix;

        // Increment total with base labels (no error_type) so success/failure paths
        // produce series with matching labels for the success_rate recording rule.
        counter!(format!("{}http_request_total", prefix), labels).increment(1);

        let mut labels = labels.clone();
        // The API forwards the proto `ErrorCode` enum verbatim (see the
        // bitping-swarm `ErrorCode` enum — single source of truth). Strip the
        // redundant `ERROR_CODE_` prefix + lowercase for dashboard ergonomics;
        // the protocol prefix (`http_`, `hls_`, `dns_`, …) survives so the
        // bucket name tells you which probe layer failed at a glance.
        //
        // New variants the node adds flow into this label automatically — there
        // is *nothing here* to keep in sync with a hand-listed taxonomy. That's
        // the whole point of carrying the typed code through.
        let error_type = error_code
            .and_then(|c| c.strip_prefix("ERROR_CODE_"))
            .map(|c| c.to_ascii_lowercase())
            .unwrap_or_else(|| {
                warn!(
                    ?error,
                    "missing error_code on failure result (old node? unmapped variant?) — bucketing as 'unknown'",
                );
                "unknown".to_string()
            });
        labels.insert("error_type", error_type);
        self.config.common_config.filter_labels(&mut labels);

        counter!(format!("{}http_request_error_total", prefix), &labels).increment(1);
    }

    fn record_success_metrics(
        &self,
        result: &PerformHttpResponseResultsItemResult,
        duration: f64,
        labels: &HashMap<&'static str, String>,
    ) {
        let prefix = &self.config.common_config.prefix;

        histogram!(format!("{}http_request_duration_ms", prefix), labels).record(duration);

        let mut status_labels = labels.clone();
        status_labels.insert("status_code", result.status_code.to_string());
        self.config.common_config.filter_labels(&mut status_labels);
        gauge!(format!("{}http_status_code", prefix), &status_labels).set(result.status_code);

        if let Some(expected_codes) = &self.config.status_codes {
            let matched = expected_codes.contains(&(result.status_code as u16));
            gauge!(format!("{}http_status_match", prefix), &status_labels).set(if matched {
                1.0
            } else {
                0.0
            });
        }

        let hash_u64 = xxh3_64(result.body_hash.as_bytes());
        gauge!(format!("{}http_body_hash", prefix), labels).set(hash_u64 as f64);

        let mut match_labels = labels.clone();
        match_labels.insert("match_count", result.matches.len().to_string());
        self.config.common_config.filter_labels(&mut match_labels);
        gauge!(format!("{}http_regex_match_count", prefix), &match_labels)
            .set(result.matches.len() as f64);

        self.record_phase_timings(result, labels);
        self.record_protocol_and_transport(result, labels);
        self.record_cdn_signals(result, labels);
        self.record_ssl_signals(result, labels);

        counter!(format!("{}http_request_success_total", prefix), labels).increment(1);
        counter!(format!("{}http_request_total", prefix), labels).increment(1);
    }

    fn record_phase_timings(
        &self,
        result: &PerformHttpResponseResultsItemResult,
        labels: &HashMap<&'static str, String>,
    ) {
        let prefix = &self.config.common_config.prefix;
        let Some(metrics) = result.metrics.as_ref() else {
            return;
        };

        if let Some(v) = metrics.dns_resolve_duration_ms {
            histogram!(format!("{}http_dns_resolve_ms", prefix), labels).record(v);
        }
        if let Some(v) = metrics.tcp_connect_duration_ms {
            histogram!(format!("{}http_tcp_connect_ms", prefix), labels).record(v);
        }
        if let Some(v) = metrics.tls_handshake_duration_ms {
            histogram!(format!("{}http_tls_handshake_ms", prefix), labels).record(v);
        }
        histogram!(format!("{}http_ttfb_ms", prefix), labels).record(metrics.http_ttfb_duration_ms);
        histogram!(format!("{}http_content_download_ms", prefix), labels)
            .record(metrics.content_download_duration_ms);
    }

    fn record_protocol_and_transport(
        &self,
        result: &PerformHttpResponseResultsItemResult,
        labels: &HashMap<&'static str, String>,
    ) {
        let prefix = &self.config.common_config.prefix;

        if let Some(proto) = result.negotiated_protocol.as_deref() {
            let bucket = normalise_protocol(proto);
            let mut proto_labels = labels.clone();
            proto_labels.insert("protocol", bucket.to_string());
            self.config.common_config.filter_labels(&mut proto_labels);
            gauge!(format!("{}http_protocol", prefix), &proto_labels).set(1.0);
        }

        if let Some(family) = result.address_family_used.as_deref() {
            let bucket = normalise_address_family(family);
            let mut family_labels = labels.clone();
            family_labels.insert("family", bucket.to_string());
            self.config.common_config.filter_labels(&mut family_labels);
            gauge!(format!("{}http_address_family", prefix), &family_labels).set(1.0);
        }

        for entry in &result.fallback_chain {
            let from_bucket = normalise_protocol(&entry.protocol).to_string();
            let to_bucket = result
                .negotiated_protocol
                .as_deref()
                .map(normalise_protocol)
                .unwrap_or("unknown")
                .to_string();
            let mut fallback_labels = labels.clone();
            fallback_labels.insert("from_protocol", from_bucket);
            fallback_labels.insert("to_protocol", to_bucket);
            self.config.common_config.filter_labels(&mut fallback_labels);
            counter!(format!("{}http_fallback_total", prefix), &fallback_labels).increment(1);
        }
    }

    fn record_cdn_signals(
        &self,
        result: &PerformHttpResponseResultsItemResult,
        labels: &HashMap<&'static str, String>,
    ) {
        let prefix = &self.config.common_config.prefix;
        let headers = &result.headers;

        let provider = parse_cdn_provider(headers);
        let mut provider_labels = labels.clone();
        provider_labels.insert("provider", provider.as_label().to_string());
        self.config.common_config.filter_labels(&mut provider_labels);
        gauge!(format!("{}http_cdn_provider", prefix), &provider_labels).set(1.0);

        if let Some(status) = parse_cache_status(headers) {
            let mut cache_labels = labels.clone();
            cache_labels.insert("status", status.to_string());
            self.config.common_config.filter_labels(&mut cache_labels);
            gauge!(format!("{}http_cache_status", prefix), &cache_labels).set(1.0);
        }

        if !matches!(provider, CdnProvider::None) {
            if let Some(pop) = parse_edge_pop(headers, provider) {
                let mut pop_labels = labels.clone();
                pop_labels.insert("pop", pop);
                self.config.common_config.filter_labels(&mut pop_labels);
                gauge!(format!("{}http_edge_pop", prefix), &pop_labels).set(1.0);
            }
        }

        if let Some(age) = parse_cache_age_seconds(headers) {
            gauge!(format!("{}http_cache_age_seconds", prefix), labels).set(age);
        }
    }

    fn record_ssl_signals(
        &self,
        result: &PerformHttpResponseResultsItemResult,
        labels: &HashMap<&'static str, String>,
    ) {
        let prefix = &self.config.common_config.prefix;
        let Some(ssl) = result.ssl_info.as_ref() else {
            return;
        };

        gauge!(format!("{}http_ssl_chain_valid", prefix), labels)
            .set(if ssl.is_valid { 1.0 } else { 0.0 });

        match DateTime::parse_from_rfc3339(&ssl.not_after) {
            Ok(expires_at) => {
                let days = (expires_at.with_timezone(&Utc) - Utc::now()).num_seconds() as f64
                    / 86_400.0;
                gauge!(
                    format!("{}http_ssl_expires_days_remaining", prefix),
                    labels
                )
                .set(days);
            }
            Err(e) => {
                warn!(?e, not_after=%ssl.not_after, "failed to parse sslInfo.notAfter as RFC3339");
            }
        }
    }
}

fn normalise_protocol(raw: &str) -> &'static str {
    let lower = raw.trim().to_ascii_lowercase();
    if lower.contains("h3") || lower.contains("quic") {
        "h3"
    } else if lower.contains("h2") {
        "h2"
    } else if lower.contains("1.1") || lower.contains("h1") || lower.contains("http/1") {
        "h1"
    } else {
        "unknown"
    }
}

fn normalise_address_family(raw: &str) -> &'static str {
    let lower = raw.trim().to_ascii_lowercase();
    if lower.contains("v6") || lower == "inet6" {
        "ipv6"
    } else if lower.contains("v4") || lower == "inet" {
        "ipv4"
    } else {
        "unknown"
    }
}
