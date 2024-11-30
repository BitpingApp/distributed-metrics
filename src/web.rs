use crate::MetricsHandle;
use prometheus::{Encoder, TextEncoder};
use tracing::info;
use warp::Filter;

#[derive(Debug)]
pub enum MetricsError {
    Generic(eyre::Report),
}

impl warp::reject::Reject for MetricsError {}

async fn metrics_handler(metrics: MetricsHandle) -> Result<impl warp::Reply, warp::Rejection> {
    let encoder = TextEncoder::new();
    let metric_families = metrics
        .gather()
        .await
        .map_err(|e| warp::reject::custom(MetricsError::Generic(e)))?;
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    Ok(String::from_utf8(buffer).unwrap())
}

pub async fn spawn_http_server(metrics_handle: MetricsHandle) {
    // Start HTTP server
    let metrics_route = warp::path!("metrics").and_then(move || {
        let metrics = metrics_handle.clone();
        async move { metrics_handler(metrics).await }
    });

    let health_route = warp::path!("health").map(|| "OK");
    let routes = metrics_route.or(health_route);

    info!("Starting metrics server on :9091");

    warp::serve(routes).run(([0, 0, 0, 0], 9091)).await
}
