//! Integration tests: exercise the real remote write code path against both
//! VictoriaMetrics and Prometheus.
//!
//! Flow: metrics crate → PrometheusHandle::render() → RemoteWriteSender::push_once() → backend → query
//!
//! Requires Docker/Podman. Run with:
//!   cargo test --test remote_write -- --ignored

mod fault_injection;
mod prometheus;
mod victoriametrics;

use std::collections::HashMap;

use distributed_metrics::config::RemoteWriteDestination;
use distributed_metrics::remote_write::RemoteWriteSender;

// ---------------------------------------------------------------------------
// Backend abstraction
// ---------------------------------------------------------------------------

pub struct RemoteWriteBackend {
    pub base_url: String,
    pub write_path: String,
    pub client: reqwest::Client,
    flush: Box<
        dyn Fn(
                &reqwest::Client,
                &str,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            + Send
            + Sync,
    >,
}

impl RemoteWriteBackend {
    pub fn make_dest(&self) -> RemoteWriteDestination {
        RemoteWriteDestination {
            name: "test".to_string(),
            url: format!("{}{}", self.base_url, self.write_path),
            username: None,
            password: None,
            headers: HashMap::new(),
            interval: std::time::Duration::from_secs(15),
            timeout: std::time::Duration::from_secs(30),
        }
    }

    /// Push rendered Prometheus text through the real `RemoteWriteSender::push_once` code path.
    pub async fn push(&self, text: &str) {
        let dest = self.make_dest();
        RemoteWriteSender::push_once(&dest, &self.client, text)
            .await
            .expect("push_once failed");
    }

    /// Flush the backend so writes become queryable. See backend-specific implementations
    /// for details on why this is needed.
    pub async fn flush(&self) {
        (self.flush)(&self.client, &self.base_url).await;
    }

    /// Execute a PromQL instant query and return the result value for the first match.
    /// Panics if the metric is not found.
    pub async fn query_value(&self, promql: &str) -> f64 {
        let result = self.query(promql).await;
        let results = Self::query_results(&result);
        assert!(
            !results.is_empty(),
            "metric not found for query: {}",
            promql
        );
        results[0]["value"][1]
            .as_str()
            .expect("value should be a string")
            .parse()
            .expect("value should be a float")
    }

    /// Execute a PromQL instant query and return the full JSON response.
    pub async fn query(&self, promql: &str) -> serde_json::Value {
        let resp = self
            .client
            .get(format!("{}/api/v1/query", self.base_url))
            .query(&[("query", promql)])
            .send()
            .await
            .expect("failed to query backend");

        resp.json::<serde_json::Value>()
            .await
            .expect("failed to parse query response")
    }

    /// Execute a PromQL instant query and return the label map for the first match.
    /// Panics if the metric is not found.
    pub async fn query_labels(&self, promql: &str) -> serde_json::Value {
        let result = self.query(promql).await;
        let results = Self::query_results(&result);
        assert!(
            !results.is_empty(),
            "metric not found for query: {}",
            promql
        );
        results[0]["metric"].clone()
    }

    /// Assert that a PromQL query returns at least one result.
    pub async fn assert_exists(&self, promql: &str) {
        let result = self.query(promql).await;
        let results = Self::query_results(&result);
        assert!(
            !results.is_empty(),
            "expected results for query: {}",
            promql
        );
    }

    pub fn query_results(response: &serde_json::Value) -> &Vec<serde_json::Value> {
        response["data"]["result"]
            .as_array()
            .expect("missing data.result in query response")
    }
}
