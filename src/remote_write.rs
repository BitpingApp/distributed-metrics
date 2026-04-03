use std::io::BufRead;

use crate::config::RemoteWriteDestination;
use eyre::Result;
use metrics_exporter_prometheus::PrometheusHandle;
use prometheus_reqwest_remote_write::{
    Label, Sample as RwSample, TimeSeries, WriteRequest, LABEL_NAME,
};
use tokio::task::JoinSet;
use std::time::Duration;
use tracing::{debug, info, warn};

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
            let backoff = if consecutive_failures > 0 {
                // Exponential backoff: base * 2^failures, capped at 5 minutes
                let multiplier = 2u64.saturating_pow(consecutive_failures.min(8));
                let backoff = base_interval.saturating_mul(multiplier as u32);
                backoff.min(Duration::from_secs(300))
            } else {
                base_interval
            };

            tokio::time::sleep(backoff).await;

            let text = {
                let h = handle.clone();
                tokio::task::spawn_blocking(move || h.render())
                    .await
                    .unwrap()
            };

            if text.is_empty() {
                debug!(dest = %dest.name, "no metrics to push");
                continue;
            }

            match Self::push_once(&dest, &client, &text).await {
                Ok(()) => {
                    if consecutive_failures > 0 {
                        info!(dest = %dest.name, "remote_write push recovered after {} failures", consecutive_failures);
                    }
                    consecutive_failures = 0;
                    debug!(dest = %dest.name, "remote_write push succeeded");
                }
                Err(e) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    let next_multiplier = 2u64.saturating_pow(consecutive_failures.min(8));
                    let next_backoff = base_interval
                        .saturating_mul(next_multiplier as u32)
                        .min(Duration::from_secs(300));
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
                // Emit each bucket as metric_name_bucket{le="..."}
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
                // Emit each quantile as metric_name{quantile="..."}
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
    if value.is_infinite() {
        "+Inf".to_string()
    } else if value.fract() == 0.0 && value.abs() < 1e15 {
        // Prometheus uses integer formatting when possible (e.g. "1" not "1.0")
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gauge() {
        let text = "# TYPE my_gauge gauge\nmy_gauge{foo=\"bar\"} 42.0\n";
        let wr = parse_text_to_write_request(text).unwrap();
        assert_eq!(wr.timeseries.len(), 1);

        let ts = &wr.timeseries[0];
        assert!(ts.labels.iter().any(|l| l.name == "__name__" && l.value == "my_gauge"));
        assert!(ts.labels.iter().any(|l| l.name == "foo" && l.value == "bar"));
        assert_eq!(ts.samples[0].value, 42.0);
    }

    #[test]
    fn test_parse_counter() {
        let text = "# TYPE http_requests_total counter\nhttp_requests_total{method=\"GET\"} 100\n";
        let wr = parse_text_to_write_request(text).unwrap();
        assert_eq!(wr.timeseries.len(), 1);
        assert_eq!(wr.timeseries[0].samples[0].value, 100.0);
    }

    #[test]
    fn test_parse_empty() {
        let wr = parse_text_to_write_request("").unwrap();
        assert!(wr.timeseries.is_empty());
    }

    #[test]
    fn test_parse_multiple_metrics() {
        let text = "\
# TYPE a gauge
a 1
# TYPE b gauge
b 2
# TYPE c gauge
c 3
";
        let wr = parse_text_to_write_request(text).unwrap();
        assert_eq!(wr.timeseries.len(), 3);
    }

    #[test]
    fn test_parse_histogram_buckets() {
        let text = "\
# HELP h A histogram
# TYPE h histogram
h_bucket{le=\"0.1\"} 10
h_bucket{le=\"0.5\"} 20
h_bucket{le=\"+Inf\"} 30
h_sum 42.5
h_count 30
";
        let wr = parse_text_to_write_request(text).unwrap();
        // 3 buckets + h_sum + h_count = 5 timeseries
        assert_eq!(wr.timeseries.len(), 5);

        // Check that bucket timeseries have the right __name__ and le labels
        let buckets: Vec<_> = wr
            .timeseries
            .iter()
            .filter(|ts| {
                ts.labels
                    .iter()
                    .any(|l| l.name == "__name__" && l.value == "h_bucket")
            })
            .collect();
        assert_eq!(buckets.len(), 3);

        // Check le labels exist
        for ts in &buckets {
            assert!(ts.labels.iter().any(|l| l.name == "le"));
        }

        // Check _sum and _count are present
        assert!(wr.timeseries.iter().any(|ts| ts
            .labels
            .iter()
            .any(|l| l.name == "__name__" && l.value == "h_sum")));
        assert!(wr.timeseries.iter().any(|ts| ts
            .labels
            .iter()
            .any(|l| l.name == "__name__" && l.value == "h_count")));
    }

    #[test]
    fn test_histogram_le_values() {
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
        let wr = parse_text_to_write_request(text).unwrap();

        let buckets: Vec<_> = wr
            .timeseries
            .iter()
            .filter(|ts| {
                ts.labels
                    .iter()
                    .any(|l| l.name == "__name__" && l.value == "h_bucket")
            })
            .collect();

        let le_values: Vec<&str> = buckets
            .iter()
            .map(|ts| {
                ts.labels
                    .iter()
                    .find(|l| l.name == "le")
                    .unwrap()
                    .value
                    .as_str()
            })
            .collect();

        assert!(le_values.contains(&"0.1"));
        assert!(le_values.contains(&"0.5"));
        assert!(le_values.contains(&"1"));
        assert!(le_values.contains(&"10"));
        assert!(le_values.contains(&"+Inf"));
    }

    #[test]
    fn test_format_le_helper() {
        assert_eq!(format_le(f64::INFINITY), "+Inf");
        assert_eq!(format_le(0.1), "0.1");
        assert_eq!(format_le(0.5), "0.5");
        assert_eq!(format_le(1.0), "1");
        assert_eq!(format_le(10.0), "10");
        assert_eq!(format_le(100.0), "100");
        assert_eq!(format_le(0.001), "0.001");
    }

    #[test]
    fn test_labels_preserved_on_histogram() {
        let text = "\
# TYPE req histogram
req_bucket{method=\"GET\",le=\"1\"} 10
req_bucket{method=\"GET\",le=\"+Inf\"} 15
req_sum{method=\"GET\"} 8.0
req_count{method=\"GET\"} 15
";
        let wr = parse_text_to_write_request(text).unwrap();
        // 2 buckets + sum + count = 4
        assert_eq!(wr.timeseries.len(), 4);

        // Every timeseries should carry the method="GET" label
        // (except _sum and _count come through as Untyped with labels already set)
        let buckets: Vec<_> = wr
            .timeseries
            .iter()
            .filter(|ts| {
                ts.labels
                    .iter()
                    .any(|l| l.name == "__name__" && l.value == "req_bucket")
            })
            .collect();
        for ts in &buckets {
            assert!(
                ts.labels
                    .iter()
                    .any(|l| l.name == "method" && l.value == "GET"),
                "bucket missing method label: {:?}",
                ts.labels
            );
        }
    }

    #[test]
    fn test_counter_with_multiple_labels() {
        let text = "\
# TYPE http_requests_total counter
http_requests_total{method=\"POST\",status=\"200\",path=\"/api\"} 42
";
        let wr = parse_text_to_write_request(text).unwrap();
        assert_eq!(wr.timeseries.len(), 1);

        let ts = &wr.timeseries[0];
        assert!(ts.labels.iter().any(|l| l.name == "__name__" && l.value == "http_requests_total"));
        assert!(ts.labels.iter().any(|l| l.name == "method" && l.value == "POST"));
        assert!(ts.labels.iter().any(|l| l.name == "status" && l.value == "200"));
        assert!(ts.labels.iter().any(|l| l.name == "path" && l.value == "/api"));
        assert_eq!(ts.samples[0].value, 42.0);
    }

    #[test]
    fn test_timestamps_are_set() {
        let text = "# TYPE g gauge\ng 1\n";
        let wr = parse_text_to_write_request(text).unwrap();
        let ts = &wr.timeseries[0];
        // Timestamp should be a reasonable epoch ms (after 2020)
        assert!(ts.samples[0].timestamp > 1_577_836_800_000);
    }

    #[test]
    fn test_encode_compressed_roundtrip() {
        let text = "# TYPE g gauge\ng{host=\"a\"} 1\n# TYPE c counter\nc 99\n";
        let wr = parse_text_to_write_request(text).unwrap();
        // Should not panic or error
        let compressed = wr.encode_compressed();
        assert!(compressed.is_ok());
        assert!(!compressed.unwrap().is_empty());
    }

    #[test]
    fn test_labels_sorted_by_name() {
        let text = "# TYPE m gauge\nm{z=\"1\",a=\"2\",m=\"3\"} 1\n";
        let wr = parse_text_to_write_request(text).unwrap();
        let ts = &wr.timeseries[0];
        let names: Vec<&str> = ts.labels.iter().map(|l| l.name.as_str()).collect();
        // After .sorted(), labels should be alphabetically ordered
        let mut sorted_names = names.clone();
        sorted_names.sort();
        assert_eq!(names, sorted_names, "labels should be sorted by name");
    }
}
