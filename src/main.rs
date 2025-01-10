use crate::config::Conf;
use collectors::icmp::IcmpCollector;
use collectors::{dns, hls, Collector};
use color_eyre::eyre::Result;
use config::MetricType;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use metrics_util::MetricKindMask;
use poem::middleware::AddData;
use poem::web::Data;
use poem::EndpointExt;
use poem::{get, handler, listener::TcpListener, Route, Server};
use progenitor::generate_api;
use std::sync::LazyLock;
use tokio::join;
use tokio::task::JoinSet;
use tracing::{error, info};

mod collectors;
mod config;

generate_api!(spec = "./api-spec.json", interface = Builder);

async fn setup() -> Result<()> {
    if std::env::var("RUST_LIB_BACKTRACE").is_err() {
        std::env::set_var("RUST_LIB_BACKTRACE", "1")
    }
    color_eyre::install()?;

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_thread_ids(true)
        .with_target(false)
        .with_thread_names(true)
        .with_ansi(true);

    if std::env::var("LOG_FMT").unwrap_or_default() == "json" {
        subscriber.json().init();
    } else {
        subscriber.pretty().init();
    }

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

static CONFIG: LazyLock<Conf> =
    LazyLock::new(|| Conf::new().expect("Failed to load configuration"));

#[handler]
fn render_prom(state: Data<&PrometheusHandle>) -> String {
    state.render()
}

#[tokio::main]
async fn main() -> Result<()> {
    setup().await?;

    info!("Starting DNS metrics collector");

    let handle = PrometheusBuilder::new()
        .idle_timeout(
            MetricKindMask::COUNTER | MetricKindMask::HISTOGRAM | MetricKindMask::GAUGE,
            Some(CONFIG.global_config.metric_clear_timeout),
        )
        .upkeep_timeout(CONFIG.global_config.metric_clear_timeout.saturating_mul(2))
        .install_recorder()
        .expect("failed to install recorder");

    let app = Route::new()
        .at("/metrics", get(render_prom))
        .with(AddData::new(handle));

    let http_server = Server::new(TcpListener::bind("[::]:3000")).run(app);

    // Start collection tasks
    let mut join_set = JoinSet::new();

    spawn_collectors(&CONFIG, &mut join_set).await?;

    let (rs, _) = join!(http_server, join_set.join_all());

    rs?;

    Ok(())
}

async fn spawn_collectors(config: &'static Conf, join_set: &mut JoinSet<()>) -> Result<()> {
    for metric in &config.metrics {
        match metric {
            MetricType::Dns(config) => {
                join_set.spawn(async move {
                    loop {
                        let collector = dns::DnsCollector::new(config);
                        if let Err(e) = collector.run().await {
                            error!("DNS collector failed: {}", e);
                        }
                    }
                });
            }
            MetricType::Icmp(config) => {
                join_set.spawn(async move {
                    loop {
                        let collector = IcmpCollector::new(config);
                        if let Err(e) = collector.run().await {
                            error!("ICMP collector failed: {}", e);
                        }
                    }
                });
            }
            MetricType::Hls(config) => {
                join_set.spawn(async move {
                    loop {
                        let collector = hls::HlsCollector::new(config);
                        if let Err(e) = collector.run().await {
                            error!("HLS collector failed: {}", e);
                        }
                    }
                });
            }
        }
    }

    Ok(())
}
