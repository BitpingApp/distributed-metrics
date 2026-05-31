//! Live end-to-end smoke test for the new CDN metrics path.
//!
//! Hits the real Bitping API, runs `HttpCollector::handle_response` on the
//! result, and asserts the rendered Prometheus snapshot contains every new
//! metric family the BIT-562 CDN MVP adds.
//!
//! Set `BITPING_API_KEY` in env and run with `--ignored`:
//!
//!     BITPING_API_KEY=... cargo test -p distributed-metrics \
//!         --test cdn_metrics_live -- --ignored --nocapture

use distributed_metrics::collectors::http::HttpCollector;
use distributed_metrics::collectors::Collector;
use distributed_metrics::config::{HttpConfig, HttpMethod, MetricConfig};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

static RECORDER: OnceLock<PrometheusHandle> = OnceLock::new();

fn install_recorder() -> &'static PrometheusHandle {
    RECORDER.get_or_init(|| {
        PrometheusBuilder::new()
            .install_recorder()
            .expect("failed to install prometheus recorder")
    })
}

fn build_http_config(
    endpoint: &str,
    name: &str,
    ssl_info: bool,
    transport: Option<&str>,
) -> &'static HttpConfig {
    let config = HttpConfig {
        common_config: MetricConfig {
            prefix: String::new(),
            name: Some(name.to_string()),
            endpoint: endpoint.to_string(),
            frequency: Duration::from_secs(30),
            network: None,
            label_whitelist: None,
        },
        headers: HashMap::new(),
        method: HttpMethod::GET,
        body: None,
        regex: None,
        status_codes: None,
        ssl_info: Some(ssl_info),
        transport: transport.map(|s| s.to_string()),
    };
    Box::leak(Box::new(config))
}

async fn probe(endpoint: &str, name: &str, expected_provider: &str) {
    let handle = install_recorder();
    let config = build_http_config(endpoint, name, true, Some("AUTO"));
    let collector = HttpCollector::new(config);
    collector.register_metrics();

    let response = collector
        .perform_request()
        .await
        .expect("perform_request to live API succeeded");

    collector
        .handle_response(response)
        .expect("handle_response without error");

    let snapshot = handle.render();
    eprintln!(
        "\n========== {name} ({endpoint}) ==========\n{snapshot}\n========== end {name} ==========\n",
    );

    let must_present = [
        ("http_request_total", true),
        ("http_request_success_total", true),
        ("http_request_duration_ms", true),
        ("http_ttfb_ms", true),
        ("http_content_download_ms", true),
        ("http_protocol", true),
        ("http_address_family", true),
        ("http_cdn_provider", true),
        ("http_status_code", true),
    ];
    let optional = [
        "http_dns_resolve_ms",
        "http_tcp_connect_ms",
        "http_tls_handshake_ms",
        "http_fallback_total",
        "http_cache_status",
        "http_edge_pop",
        "http_cache_age_seconds",
        "http_ssl_expires_days_remaining",
        "http_ssl_chain_valid",
    ];

    let mut missing = Vec::new();
    let mut seen = Vec::new();
    for (metric, required) in must_present {
        if snapshot.contains(metric) {
            seen.push(metric);
        } else if required {
            missing.push(metric);
        }
    }
    let mut optional_seen = Vec::new();
    for metric in optional {
        if snapshot.contains(metric) {
            optional_seen.push(metric);
        }
    }
    eprintln!("[{name}] required seen: {seen:?}");
    eprintln!("[{name}] optional seen: {optional_seen:?}");

    let provider_marker = format!("provider=\"{expected_provider}\"");
    assert!(
        snapshot.contains(&provider_marker),
        "[{name}] expected http_cdn_provider with provider=\"{expected_provider}\" — snapshot did not contain `{provider_marker}`"
    );

    if !missing.is_empty() {
        panic!("[{name}] required metric families missing from snapshot: {missing:?}");
    }
}

#[tokio::test]
#[ignore]
async fn cloudflare_probe_emits_full_cdn_metric_set() {
    probe("https://cloudflare.com", "cloudflare-home", "cloudflare").await;
}

#[tokio::test]
#[ignore]
async fn cloudfront_probe_detects_aws() {
    probe("https://aws.amazon.com", "aws-home", "cloudfront").await;
}

#[tokio::test]
#[ignore]
async fn vercel_probe_detects_vercel() {
    probe("https://vercel.com", "vercel-home", "vercel").await;
}
