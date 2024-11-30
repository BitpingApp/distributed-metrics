use crate::config::{Conf, MetricConfig, MetricType};
use collector::{metrics_processor, LocationInfo, MetricUpdate, MetricsHandle};
use color_eyre::eyre::{Context, Result};
use progenitor::generate_api;
use prometheus::{Counter, CounterVec, Gauge, GaugeVec, Opts};
use prometheus::{Encoder, Registry, TextEncoder};
use reqwest::StatusCode;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, LazyLock};
use tokio::join;
use tokio::sync::{mpsc, oneshot};
use tokio::task::LocalSet;
use tracing::{debug, error, info, instrument, warn};
use tracing_subscriber::Layer;
use types::{
    PerformDnsBodyContinentCode, PerformDnsBodyCountryCode, PerformDnsBodyMobile,
    PerformDnsBodyProxy, PerformDnsBodyResidential,
};
use web::spawn_http_server;

mod collector;
mod config;
mod metrics;
mod web;

generate_api!(spec = "./api-spec.json", interface = Builder);

async fn setup() -> Result<()> {
    if std::env::var("RUST_LIB_BACKTRACE").is_err() {
        std::env::set_var("RUST_LIB_BACKTRACE", "1")
    }
    color_eyre::install()?;

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .pretty()
        .with_thread_ids(true)
        .with_target(false)
        .with_thread_names(true)
        .with_ansi(true)
        .init();

    Ok(())
}

static API_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-api-key",
        HeaderValue::try_from(std::env::var("BITPING_API_KEY").expect("Couldn't get API key"))
            .unwrap(),
    );

    let req_client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    Client::new_with_client("https://api.bitping.com/v2", req_client)
});

#[tokio::main]
async fn main() -> Result<()> {
    setup().await?;

    info!("Starting DNS metrics collector");
    let config = Conf::new().context("Failed to load configuration")?;

    // Create metrics handle
    let (metrics_handle, rx) = MetricsHandle::new()?;

    // Spawn metrics processor
    tokio::spawn(async move {
        if let Err(e) = metrics_processor(rx).await {
            error!("Metrics processor error: {}", e);
        }
    });

    // Start collection tasks
    let local_set = LocalSet::new();
    spawn_collectors(config, metrics_handle.clone(), &local_set).await?;

    let http_server = spawn_http_server(metrics_handle);

    // Drive both
    join!(http_server, local_set);

    Ok(())
}

async fn spawn_collectors(
    config: Conf,
    metrics: MetricsHandle,
    local_set: &LocalSet,
) -> Result<()> {
    for metric in config.metric {
        if matches!(metric.metric_type, MetricType::Dns) {
            debug!("Setting up collection for prefix: {}", metric.prefix);
            let frequency = tokio::time::Duration::from_millis(metric.frequency_ms);

            let metric_config = metric.clone();
            let metrics = metrics.clone();
            local_set.spawn_local(async move {
                loop {
                    let country_code = metric_config
                        .network
                        .as_ref()
                        .and_then(|x| x.country_code)
                        .map(|c| c.to_alpha2().to_string())
                        .and_then(|x| PerformDnsBodyCountryCode::from_str(&x).ok());

                    let continent_code = metric_config
                        .network
                        .as_ref()
                        .and_then(|x| x.continent_code.clone())
                        .and_then(|c| PerformDnsBodyContinentCode::from_str(c.as_ref()).ok());

                    let mobile = metric_config
                        .network
                        .as_ref()
                        .map(|n| n.mobile.as_ref().to_uppercase())
                        .and_then(|mo| PerformDnsBodyMobile::from_str(&mo).ok())
                        .unwrap_or_else(PerformDnsBodyMobile::default);

                    let residential = metric_config
                        .network
                        .as_ref()
                        .map(|n| n.residential.as_ref().to_uppercase())
                        .and_then(|mo| PerformDnsBodyResidential::from_str(&mo).ok())
                        .unwrap_or_else(PerformDnsBodyResidential::default);

                    let proxy = metric_config
                        .network
                        .as_ref()
                        .map(|n| n.proxy.as_ref().to_uppercase())
                        .and_then(|mo| PerformDnsBodyProxy::from_str(&mo).ok())
                        .unwrap_or_else(PerformDnsBodyProxy::default);

                    info!(?metric_config, ?country_code, "Sending request");

                    match API_CLIENT
                        .perform_dns()
                        .body_map(|body| {
                            body.hostnames([metric_config.endpoint.clone()])
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

                            // Safe array access
                            if let Some(result) = response.results.first() {
                                // Handle optional fields safely
                                if let (Some(dns_result), Some(node_info)) =
                                    (result.result.clone(), response.node_info)
                                {
                                    // Safely handle optional fields from node_info
                                        let _ = metrics
                                            .update_metrics(MetricUpdate {
                                                prefix: metric_config.prefix.clone(),
                                                location: LocationInfo {
                                                    continent: node_info.continent_code.clone(),
                                                    country_code: node_info.country_code.clone(),
                                                    city:node_info.city.clone(),
                                                    isp: node_info.isp.clone(),
                                                },
                                                record_type: "IP".into(),
                                                duration: result.duration.unwrap_or(0.0),
                                                success: true,
                                                record_count: dns_result.ips.len() as u64,
                                                ttl: 0,
                                            })
                                            .await;
                                } else {
                                    error!("Missing DNS result or node info");
                                }
                            } else {
                                error!("No results returned from API");
                            }
                        }
                        Err(e) => {
                            if let Some(StatusCode::NOT_FOUND) = e.status() {
                                warn!(?metric_config.network,"No nodes were found for the given criteria");
                            } else {
                                error!("API request failed: {:#?}", e);
                            }
                        }
                    }

                    tokio::time::sleep(frequency).await;
                }
            });
        }
    }

    Ok(())
}
