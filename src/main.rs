use crate::config::{Conf, MetricType};
use collectors::runner::run_collector;
use collectors::{dns, hls, icmp, Collector};
use color_eyre::eyre::{Context, Result};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use poem::middleware::AddData;
use poem::web::Data;
use poem::EndpointExt;
use poem::{get, handler, listener::TcpListener, Route, Server};
use progenitor::generate_api;
use std::sync::LazyLock;
use tokio::join;
use tokio::task::LocalSet;
use tracing::{error, info};

mod collectors;
mod config;

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

#[handler]
fn render_prom(state: Data<&PrometheusHandle>) -> String {
    state.render()
}

#[tokio::main]
async fn main() -> Result<()> {
    setup().await?;

    info!("Starting DNS metrics collector");
    let config = Conf::new().context("Failed to load configuration")?;

    let builder = PrometheusBuilder::new();
    let handle = builder
        .install_recorder()
        .expect("failed to install recorder");

    let app = Route::new()
        .at("/metrics", get(render_prom))
        .with(AddData::new(handle));

    let http_server = Server::new(TcpListener::bind("0.0.0.0:3000")).run(app);

    // Start collection tasks
    let local_set = LocalSet::new();

    spawn_collectors(config, &local_set).await?;

    join!(http_server, local_set);

    Ok(())
}

async fn spawn_collectors(config: Conf, local_set: &LocalSet) -> Result<()> {
    for metric in config.metrics {
        match metric.metric_type {
            MetricType::Dns => {
                let collector = dns::DnsCollector::new(metric);
                local_set.spawn_local(async move {
                    if let Err(e) = run_collector(collector).await {
                        error!("DNS collector failed: {}", e);
                    }
                });
            }
            MetricType::Icmp => {
                let collector = icmp::IcmpCollector::new(metric);
                local_set.spawn_local(async move {
                    if let Err(e) = run_collector(collector).await {
                        error!("ICMP collector failed: {}", e);
                    }
                });
            }
            MetricType::Hls => {
                let collector = hls::HlsCollector::new(metric);
                local_set.spawn_local(async move {
                    if let Err(e) = run_collector(collector).await {
                        error!("ICMP collector failed: {}", e);
                    }
                });
            }
        }
    }

    Ok(())
}
