use std::io::BufRead;
use std::time::Duration;

use crate::config::RemoteWriteDestination;
use eyre::Result;
use metrics_exporter_prometheus::PrometheusHandle;
use prometheus_reqwest_remote_write::{
    Label, Sample as RwSample, TimeSeries, WriteRequest, LABEL_NAME,
};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

/// Maximum backoff duration between retries (5 minutes).
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Maximum exponent for 2^n to prevent overflow in backoff calculation.
const MAX_BACKOFF_EXPONENT: u32 = 8;

pub struct RemoteWriteSender {
    destinations: Vec<RemoteWriteDestination>,
    handle: PrometheusHandle,
    client: reqwest::Client,
}

impl RemoteWriteSender {
    pub fn new(destinations: Vec<RemoteWriteDestination>, handle: PrometheusHandle) -> Self {
        Self {
            destinations,
            handle,
            client: reqwest::Client::new(),
        }
    }

    pub fn spawn_all(self, join_set: &mut JoinSet<()>) {
        for dest in self.destinations {
            let handle = self.handle.clone();
            let client = self.client.clone();
            join_set.spawn(async move {
                Self::push_loop(dest, handle, client).await;
            });
        }
    }

    async fn push_loop(
        dest: RemoteWriteDestination,
        handle: PrometheusHandle,
        client: reqwest::Client,
    ) {
        let base_interval = dest.interval;
        let mut consecutive_failures: u32 = 0;

        loop {
            let backoff = calculate_backoff(base_interval, consecutive_failures);
            tokio::time::sleep(backoff).await;

            let text = match render_metrics(&handle).await {
                Ok(text) => text,
                Err(e) => {
                    error!(dest = %dest.name, error = %e, "failed to render metrics");
                    continue;
                }
            };

            if text.is_empty() {
                debug!(dest = %dest.name, "no metrics to push");
                continue;
            }

            match Self::push_once(&dest, &client, &text).await {
                Ok(()) => {
                    if consecutive_failures > 0 {
                        info!(
                            dest = %dest.name,
                            "remote_write push recovered after {} failures",
                            consecutive_failures
                        );
                    }
                    consecutive_failures = 0;
                    debug!(dest = %dest.name, "remote_write push succeeded");
                }
                Err(e) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    let next_backoff =
                        calculate_backoff(base_interval, consecutive_failures);
                    warn!(
                        dest = %dest.name,
                        error = %e,
                        consecutive_failures,
                        next_retry_secs = next_backoff.as_secs(),
                        "remote_write push failed"
                    );
                }
            }
        }
    }

    pub async fn push_once(
        dest: &RemoteWriteDestination,
        client: &reqwest::Client,
        text: &str,
    ) -> Result<()> {
        let write_request = parse_text_to_write_request(text)?;
        let body = write_request
            .encode_compressed()
            .map_err(|e| eyre::eyre!("snappy compression failed: {}", e))?;

        let mut req = client
            .post(&dest.url)
            .timeout(dest.timeout)
            .header("Content-Type", "application/x-protobuf")
            .header("Content-Encoding", "snappy")
            .header("X-Prometheus-Remote-Write-Version", "0.1.0")
            .body(body);

        for (key, value) in &dest.headers {
            req = req.header(key, value);
        }

        if let Some(ref username) = dest.username {
            req = req.basic_auth(username, dest.password.as_deref());
        }

        let resp = req.send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let mut body = resp.text().await.unwrap_or_default();
            body.truncate(1024);
            return Err(eyre::eyre!("remote_write returned {}: {}", status, body));
        }
        Ok(())
    }
}

/// Render metrics from the PrometheusHandle in a blocking task.
async fn render_metrics(handle: &PrometheusHandle) -> Result<String> {
    let h = handle.clone();
    tokio::task::spawn_blocking(move || h.render())
        .await
        .map_err(|e| eyre::eyre!("render task panicked: {}", e))
}

/// Calculate the backoff duration for a given number of consecutive failures.
/// Uses exponential backoff: base_interval * 2^failures, capped at MAX_BACKOFF.
/// Returns base_interval when there are no failures.
pub fn calculate_backoff(base_interval: Duration, consecutive_failures: u32) -> Duration {
    if consecutive_failures == 0 {
        return base_interval;
    }
    let multiplier = 2u64.saturating_pow(consecutive_failures.min(MAX_BACKOFF_EXPONENT));
    let backoff = base_interval.saturating_mul(multiplier as u32);
    backoff.min(MAX_BACKOFF)
}

pub fn parse_text_to_write_request(text: &str) -> Result<WriteRequest> {
    let reader = std::io::BufReader::new(text.as_bytes());
    let scrape = prometheus_parse::Scrape::parse(reader.lines())
        .map_err(|e| eyre::eyre!("failed to parse prometheus text: {}", e))?;

    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut timeseries: Vec<TimeSeries> = Vec::new();

    for sample in &scrape.samples {
        let base_labels: Vec<Label> = sample
            .labels
            .iter()
            .map(|(k, v)| Label {
                name: k.clone(),
                value: v.clone(),
            })
            .collect();

        match &sample.value {
            prometheus_parse::Value::Counter(v)
            | prometheus_parse::Value::Gauge(v)
            | prometheus_parse::Value::Untyped(v) => {
                let mut labels = vec![Label {
                    name: LABEL_NAME.to_string(),
                    value: sample.metric.clone(),
                }];
                labels.extend(base_labels);
                timeseries.push(TimeSeries {
                    labels,
                    samples: vec![RwSample {
                        value: *v,
                        timestamp: now_ms,
                    }],
                });
            }
            prometheus_parse::Value::Histogram(counts) => {
                for bucket in counts {
                    let mut labels = vec![
                        Label {
                            name: LABEL_NAME.to_string(),
                            value: format!("{}_bucket", sample.metric),
                        },
                        Label {
                            name: "le".to_string(),
                            value: format_le(bucket.less_than),
                        },
                    ];
                    labels.extend(base_labels.clone());
                    timeseries.push(TimeSeries {
                        labels,
                        samples: vec![RwSample {
                            value: bucket.count,
                            timestamp: now_ms,
                        }],
                    });
                }
            }
            prometheus_parse::Value::Summary(counts) => {
                for quantile in counts {
                    let mut labels = vec![
                        Label {
                            name: LABEL_NAME.to_string(),
                            value: sample.metric.clone(),
                        },
                        Label {
                            name: "quantile".to_string(),
                            value: quantile.quantile.to_string(),
                        },
                    ];
                    labels.extend(base_labels.clone());
                    timeseries.push(TimeSeries {
                        labels,
                        samples: vec![RwSample {
                            value: quantile.count,
                            timestamp: now_ms,
                        }],
                    });
                }
            }
        }
    }

    Ok(WriteRequest { timeseries }.sorted())
}

/// Format the `le` label value for histogram buckets, matching Prometheus conventions.
fn format_le(value: f64) -> String {
    if value.is_infinite() && value.is_sign_positive() {
        "+Inf".to_string()
    } else if value.is_infinite() {
        "-Inf".to_string()
    } else if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // calculate_backoff
    // -----------------------------------------------------------------------

    #[test]
    fn backoff_zero_failures_returns_base() {
        // Arrange
        let base = Duration::from_secs(15);

        // Act
        let result = calculate_backoff(base, 0);

        // Assert
        assert_eq!(result, base);
    }

    #[test]
    fn backoff_one_failure_doubles() {
        // Arrange
        let base = Duration::from_secs(10);

        // Act
        let result = calculate_backoff(base, 1);

        // Assert
        assert_eq!(result, Duration::from_secs(20));
    }

    #[test]
    fn backoff_three_failures_is_8x() {
        // Arrange
        let base = Duration::from_secs(5);

        // Act
        let result = calculate_backoff(base, 3);

        // Assert
        assert_eq!(result, Duration::from_secs(40)); // 5 * 2^3 = 40
    }

    #[test]
    fn backoff_caps_at_max() {
        // Arrange
        let base = Duration::from_secs(15);

        // Act
        let result = calculate_backoff(base, 100);

        // Assert
        assert_eq!(result, MAX_BACKOFF);
    }

    #[test]
    fn backoff_exponent_capped_at_8() {
        // Arrange
        let base = Duration::from_secs(1);

        // Act — failures 8, 9, 10 should all produce the same multiplier (256)
        let at_8 = calculate_backoff(base, 8);
        let at_9 = calculate_backoff(base, 9);
        let at_20 = calculate_backoff(base, 20);

        // Assert
        assert_eq!(at_8, Duration::from_secs(256));
        assert_eq!(at_9, Duration::from_secs(256));
        assert_eq!(at_20, Duration::from_secs(256));
    }

    #[test]
    fn backoff_large_base_saturates_to_max() {
        // Arrange — base of 60s * 256 = 15360s, but capped at 300s
        let base = Duration::from_secs(60);

        // Act
        let result = calculate_backoff(base, 8);

        // Assert
        assert_eq!(result, MAX_BACKOFF);
    }

    #[test]
    fn backoff_u32_max_failures_does_not_panic() {
        // Arrange
        let base = Duration::from_secs(1);

        // Act — should not overflow or panic
        let result = calculate_backoff(base, u32::MAX);

        // Assert
        assert_eq!(result, Duration::from_secs(256).min(MAX_BACKOFF));
    }

    // -----------------------------------------------------------------------
    // format_le
    // -----------------------------------------------------------------------

    #[test]
    fn format_le_infinity() {
        assert_eq!(format_le(f64::INFINITY), "+Inf");
    }

    #[test]
    fn format_le_negative_infinity() {
        assert_eq!(format_le(f64::NEG_INFINITY), "-Inf");
    }

    #[test]
    fn format_le_whole_numbers_as_integers() {
        assert_eq!(format_le(1.0), "1");
        assert_eq!(format_le(10.0), "10");
        assert_eq!(format_le(100.0), "100");
        assert_eq!(format_le(0.0), "0");
    }

    #[test]
    fn format_le_fractional_numbers() {
        assert_eq!(format_le(0.1), "0.1");
        assert_eq!(format_le(0.5), "0.5");
        assert_eq!(format_le(0.001), "0.001");
    }

    #[test]
    fn format_le_nan() {
        // NaN should pass through as the float's string representation
        let result = format_le(f64::NAN);
        assert_eq!(result, "NaN");
    }

    // -----------------------------------------------------------------------
    // parse_text_to_write_request — basic types
    // -----------------------------------------------------------------------

    #[test]
    fn parse_gauge() {
        // Arrange
        let text = "# TYPE my_gauge gauge\nmy_gauge{foo=\"bar\"} 42.0\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 1);
        let ts = &wr.timeseries[0];
        assert!(ts.labels.iter().any(|l| l.name == "__name__" && l.value == "my_gauge"));
        assert!(ts.labels.iter().any(|l| l.name == "foo" && l.value == "bar"));
        assert_eq!(ts.samples[0].value, 42.0);
    }

    #[test]
    fn parse_counter() {
        // Arrange
        let text = "# TYPE http_requests_total counter\nhttp_requests_total{method=\"GET\"} 100\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 1);
        assert_eq!(wr.timeseries[0].samples[0].value, 100.0);
    }

    #[test]
    fn parse_empty_input() {
        // Arrange / Act
        let wr = parse_text_to_write_request("").expect("parse failed");

        // Assert
        assert!(wr.timeseries.is_empty());
    }

    #[test]
    fn parse_multiple_metrics() {
        // Arrange
        let text = "# TYPE a gauge\na 1\n# TYPE b gauge\nb 2\n# TYPE c gauge\nc 3\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 3);
    }

    #[test]
    fn parse_counter_with_multiple_labels() {
        // Arrange
        let text = "# TYPE http_requests_total counter\nhttp_requests_total{method=\"POST\",status=\"200\",path=\"/api\"} 42\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 1);
        let ts = &wr.timeseries[0];
        assert!(ts.labels.iter().any(|l| l.name == "__name__" && l.value == "http_requests_total"));
        assert!(ts.labels.iter().any(|l| l.name == "method" && l.value == "POST"));
        assert!(ts.labels.iter().any(|l| l.name == "status" && l.value == "200"));
        assert!(ts.labels.iter().any(|l| l.name == "path" && l.value == "/api"));
        assert_eq!(ts.samples[0].value, 42.0);
    }

    // -----------------------------------------------------------------------
    // parse_text_to_write_request — histograms
    // -----------------------------------------------------------------------

    #[test]
    fn parse_histogram_emits_buckets_sum_count() {
        // Arrange
        let text = "\
# HELP h A histogram
# TYPE h histogram
h_bucket{le=\"0.1\"} 10
h_bucket{le=\"0.5\"} 20
h_bucket{le=\"+Inf\"} 30
h_sum 42.5
h_count 30
";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert — 3 buckets + h_sum + h_count = 5 timeseries
        assert_eq!(wr.timeseries.len(), 5);

        let buckets: Vec<_> = wr.timeseries.iter()
            .filter(|ts| ts.labels.iter().any(|l| l.name == "__name__" && l.value == "h_bucket"))
            .collect();
        assert_eq!(buckets.len(), 3);
        for ts in &buckets {
            assert!(ts.labels.iter().any(|l| l.name == "le"));
        }

        assert!(wr.timeseries.iter().any(|ts| ts.labels.iter().any(|l| l.name == "__name__" && l.value == "h_sum")));
        assert!(wr.timeseries.iter().any(|ts| ts.labels.iter().any(|l| l.name == "__name__" && l.value == "h_count")));
    }

    #[test]
    fn parse_histogram_le_values_correct() {
        // Arrange
        let text = "\
# TYPE h histogram
h_bucket{le=\"0.1\"} 1
h_bucket{le=\"0.5\"} 2
h_bucket{le=\"1\"} 3
h_bucket{le=\"10\"} 4
h_bucket{le=\"+Inf\"} 5
h_sum 7.5
h_count 5
";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        let buckets: Vec<_> = wr.timeseries.iter()
            .filter(|ts| ts.labels.iter().any(|l| l.name == "__name__" && l.value == "h_bucket"))
            .collect();
        let le_values: Vec<&str> = buckets.iter()
            .filter_map(|ts| ts.labels.iter().find(|l| l.name == "le").map(|l| l.value.as_str()))
            .collect();
        assert!(le_values.contains(&"0.1"));
        assert!(le_values.contains(&"0.5"));
        assert!(le_values.contains(&"1"));
        assert!(le_values.contains(&"10"));
        assert!(le_values.contains(&"+Inf"));
    }

    #[test]
    fn parse_histogram_preserves_extra_labels() {
        // Arrange
        let text = "\
# TYPE req histogram
req_bucket{method=\"GET\",le=\"1\"} 10
req_bucket{method=\"GET\",le=\"+Inf\"} 15
req_sum{method=\"GET\"} 8.0
req_count{method=\"GET\"} 15
";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 4);
        let buckets: Vec<_> = wr.timeseries.iter()
            .filter(|ts| ts.labels.iter().any(|l| l.name == "__name__" && l.value == "req_bucket"))
            .collect();
        for ts in &buckets {
            assert!(
                ts.labels.iter().any(|l| l.name == "method" && l.value == "GET"),
                "bucket missing method label: {:?}", ts.labels
            );
        }
    }

    // -----------------------------------------------------------------------
    // parse_text_to_write_request — edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn parse_only_comments_and_help() {
        // Arrange
        let text = "# HELP my_metric A helpful metric\n# TYPE my_metric gauge\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert!(wr.timeseries.is_empty());
    }

    #[test]
    fn parse_metric_without_labels() {
        // Arrange
        let text = "# TYPE up gauge\nup 1\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 1);
        let ts = &wr.timeseries[0];
        assert_eq!(ts.labels.len(), 1); // only __name__
        assert!(ts.labels.iter().any(|l| l.name == "__name__" && l.value == "up"));
    }

    #[test]
    fn parse_untyped_metric() {
        // Arrange — no # TYPE line means Untyped
        let text = "my_metric 123.456\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 1);
        assert_eq!(wr.timeseries[0].samples[0].value, 123.456);
    }

    #[test]
    fn parse_nan_value() {
        // Arrange
        let text = "# TYPE g gauge\ng NaN\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 1);
        assert!(wr.timeseries[0].samples[0].value.is_nan());
    }

    #[test]
    fn parse_positive_infinity_value() {
        // Arrange
        let text = "# TYPE g gauge\ng +Inf\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 1);
        assert!(wr.timeseries[0].samples[0].value.is_infinite());
        assert!(wr.timeseries[0].samples[0].value.is_sign_positive());
    }

    #[test]
    fn parse_negative_infinity_value() {
        // Arrange
        let text = "# TYPE g gauge\ng -Inf\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 1);
        assert!(wr.timeseries[0].samples[0].value.is_infinite());
        assert!(wr.timeseries[0].samples[0].value.is_sign_negative());
    }

    #[test]
    fn parse_zero_value() {
        // Arrange
        let text = "# TYPE g gauge\ng 0\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries[0].samples[0].value, 0.0);
    }

    #[test]
    fn parse_negative_value() {
        // Arrange
        let text = "# TYPE g gauge\ng -42.5\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries[0].samples[0].value, -42.5);
    }

    #[test]
    fn parse_scientific_notation() {
        // Arrange
        let text = "# TYPE g gauge\ng 1.23e4\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries[0].samples[0].value, 12300.0);
    }

    #[test]
    fn parse_label_with_special_characters() {
        // Arrange
        let text = "# TYPE m gauge\nm{path=\"/api/v1/users\",method=\"GET\"} 1\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert!(wr.timeseries[0].labels.iter().any(|l| l.name == "path" && l.value == "/api/v1/users"));
    }

    #[test]
    fn parse_label_with_escaped_quotes() {
        // Arrange
        let text = "# TYPE m gauge\nm{msg=\"hello \\\"world\\\"\"} 1\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 1);
    }

    // -----------------------------------------------------------------------
    // parse_text_to_write_request — output properties
    // -----------------------------------------------------------------------

    #[test]
    fn timestamps_are_recent() {
        // Arrange
        let text = "# TYPE g gauge\ng 1\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert — timestamp should be a reasonable epoch ms (after 2020)
        assert!(wr.timeseries[0].samples[0].timestamp > 1_577_836_800_000);
    }

    #[test]
    fn encode_compressed_produces_nonempty_bytes() {
        // Arrange
        let text = "# TYPE g gauge\ng{host=\"a\"} 1\n# TYPE c counter\nc 99\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");
        let compressed = wr.encode_compressed();

        // Assert
        assert!(compressed.is_ok());
        assert!(!compressed.expect("encode failed").is_empty());
    }

    #[test]
    fn labels_are_sorted_by_name() {
        // Arrange
        let text = "# TYPE m gauge\nm{z=\"1\",a=\"2\",m=\"3\"} 1\n";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert
        let ts = &wr.timeseries[0];
        let names: Vec<&str> = ts.labels.iter().map(|l| l.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "labels should be sorted by name");
    }

    #[test]
    fn high_cardinality_input() {
        // Arrange — 100 distinct timeseries
        let mut text = String::from("# TYPE m gauge\n");
        for i in 0..100 {
            text.push_str(&format!("m{{id=\"{}\"}} {}\n", i, i as f64));
        }

        // Act
        let wr = parse_text_to_write_request(&text).expect("parse failed");

        // Assert
        assert_eq!(wr.timeseries.len(), 100);
    }

    #[test]
    fn summary_emits_quantile_timeseries() {
        // Arrange
        let text = "\
# TYPE rpc_duration summary
rpc_duration{quantile=\"0.5\"} 0.05
rpc_duration{quantile=\"0.99\"} 0.5
rpc_duration_sum 100
rpc_duration_count 200
";

        // Act
        let wr = parse_text_to_write_request(text).expect("parse failed");

        // Assert — 2 quantiles + _sum + _count = 4
        assert_eq!(wr.timeseries.len(), 4);
        let quantiles: Vec<_> = wr.timeseries.iter()
            .filter(|ts| ts.labels.iter().any(|l| l.name == "quantile"))
            .collect();
        assert_eq!(quantiles.len(), 2);
    }
}
