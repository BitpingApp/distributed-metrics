use super::RemoteWriteBackend;
use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use testcontainers::core::ImageExt;
use testcontainers::{core::IntoContainerPort, runners::AsyncRunner, GenericImage};

async fn start() -> (
    testcontainers::ContainerAsync<GenericImage>,
    RemoteWriteBackend,
) {
    let container = GenericImage::new("victoriametrics/victoria-metrics", "latest")
        .with_exposed_port(8428.tcp())
        .with_wait_for(testcontainers::core::WaitFor::seconds(2))
        .with_cmd([
            "-search.latencyOffset=0s",
            "-search.maxStalenessInterval=0s",
        ])
        .start()
        .await
        .expect("failed to start VictoriaMetrics");

    let host = container.get_host().await.expect("failed to get host");
    let port = container
        .get_host_port_ipv4(8428.tcp())
        .await
        .expect("failed to get port");
    let base_url = format!("http://{}:{}", host, port);

    let backend = RemoteWriteBackend {
        base_url,
        write_path: "/api/v1/write".to_string(),
        client: reqwest::Client::new(),
        // VictoriaMetrics buffers incoming data and flushes to storage asynchronously.
        // Without an explicit flush, queries immediately after a write may return empty
        // results. This calls VM's `/internal/force_flush` endpoint and waits briefly
        // for indexing to complete. This is a test-only concern — production remote
        // write clients don't need to flush the receiving end.
        flush: Box::new(|client, base_url| {
            let client = client.clone();
            let base_url = base_url.to_string();
            Box::pin(async move {
                let _ = client
                    .get(format!("{}/internal/force_flush", base_url))
                    .send()
                    .await;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            })
        }),
    };

    (container, backend)
}

#[tokio::test]
#[ignore = "Requires Docker/Podman"]
async fn gauge_and_counter() {
    // Arrange
    let (_container, backend) = start().await;
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    // Act
    metrics::with_local_recorder(&recorder, || {
        gauge!("vm_test_gauge", "env" => "test", "host" => "ci").set(42.0);
        counter!("vm_test_requests_total", "method" => "GET", "status" => "200").increment(17);
    });
    backend.push(&handle.render()).await;
    backend.flush().await;

    // Assert
    assert_eq!(backend.query_value("vm_test_gauge").await, 42.0);
    let labels = backend.query_labels("vm_test_gauge").await;
    assert_eq!(labels["env"], "test");
    assert_eq!(labels["host"], "ci");

    assert_eq!(
        backend
            .query_value(r#"vm_test_requests_total{method="GET"}"#)
            .await,
        17.0
    );
    let labels = backend
        .query_labels(r#"vm_test_requests_total{method="GET"}"#)
        .await;
    assert_eq!(labels["status"], "200");
}

#[tokio::test]
#[ignore = "Requires Docker/Podman"]
async fn histogram() {
    // Arrange
    let (_container, backend) = start().await;
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let expected_sum = 0.05 + 0.15 + 0.5 + 1.5 + 5.0;

    // Act
    metrics::with_local_recorder(&recorder, || {
        let h = histogram!("vm_test_duration_seconds", "endpoint" => "/api");
        h.record(0.05);
        h.record(0.15);
        h.record(0.5);
        h.record(1.5);
        h.record(5.0);
    });
    backend.push(&handle.render()).await;
    backend.flush().await;

    // Assert
    let sum = backend.query_value("vm_test_duration_seconds_sum").await;
    assert!(
        (sum - expected_sum).abs() < 0.001,
        "expected sum ~{}, got {}",
        expected_sum,
        sum
    );
    assert_eq!(
        backend.query_value("vm_test_duration_seconds_count").await,
        5.0
    );
    let labels = backend
        .query_labels(r#"vm_test_duration_seconds{quantile="0.5"}"#)
        .await;
    assert_eq!(labels["endpoint"], "/api");
}

#[tokio::test]
#[ignore = "Requires Docker/Podman"]
async fn multiple_pushes() {
    // Arrange
    let (_container, backend) = start().await;
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    // Act
    metrics::with_local_recorder(&recorder, || {
        counter!("vm_test_push_counter").increment(10);
    });
    backend.push(&handle.render()).await;

    metrics::with_local_recorder(&recorder, || {
        counter!("vm_test_push_counter").increment(5);
    });
    backend.push(&handle.render()).await;
    backend.flush().await;

    // Assert
    assert_eq!(backend.query_value("vm_test_push_counter").await, 15.0);
}
