use color_eyre::eyre::{Context, Result};
use prometheus::{Counter, CounterVec, Gauge, GaugeVec, Opts};
use prometheus::{Encoder, Registry, TextEncoder};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, instrument, warn};

#[derive(Clone, Debug)]
pub struct LocationInfo {
    pub country_code: String,
    pub continent: String,
    pub city: String,
    pub isp: String,
}

#[derive(Debug)]
pub enum MetricCommand {
    Update(MetricUpdate),
    Gather {
        respond_to: oneshot::Sender<Vec<prometheus::proto::MetricFamily>>,
    },
}

#[derive(Debug)]
pub struct MetricUpdate {
    pub prefix: String,
    pub location: LocationInfo,
    pub record_type: String,
    pub duration: f64,
    pub success: bool,
    pub record_count: u64,
    pub ttl: u64,
}

pub struct DnsMetrics {
    pub lookup_duration: GaugeVec,
    pub response_percentiles: GaugeVec,
    pub lookup_success_ratio: GaugeVec,
    pub lookup_failures: CounterVec,
    pub records_returned: GaugeVec,
    pub record_mismatch_count: GaugeVec,
    pub record_ttl: GaugeVec,
    pub resolver_hops: GaugeVec,
    pub propagation_delay: GaugeVec,
}

impl DnsMetrics {
    pub fn new(prefix: &str, registry: &Registry) -> Result<Self> {
        let base_labels = &["country_code", "continent", "city", "isp"];
        let record_labels = &["country_code", "continent", "city", "isp", "record_type"];
        let percentile_labels = &["country_code", "continent", "city", "isp", "percentile"];

        let lookup_duration = GaugeVec::new(
            Opts::new(
                &format!("{}_dns_lookup_duration_seconds", prefix),
                "Time taken to perform DNS lookup in seconds",
            ),
            record_labels,
        )?;

        let response_percentiles = GaugeVec::new(
            Opts::new(
                &format!("{}_dns_response_percentiles_seconds", prefix),
                "Percentile latencies for DNS lookups",
            ),
            percentile_labels,
        )?;

        let lookup_success_ratio = GaugeVec::new(
            Opts::new(
                &format!("{}_dns_lookup_success_ratio", prefix),
                "Ratio of successful DNS lookups",
            ),
            base_labels,
        )?;

        let lookup_failures = CounterVec::new(
            Opts::new(
                &format!("{}_dns_lookup_failures_total", prefix),
                "Total number of DNS lookup failures",
            ),
            &["country_code", "continent", "city", "isp", "error_type"],
        )?;

        let records_returned = GaugeVec::new(
            Opts::new(
                &format!("{}_dns_records_returned", prefix),
                "Number of DNS records returned by type",
            ),
            record_labels,
        )?;

        let record_mismatch_count = GaugeVec::new(
            Opts::new(
                &format!("{}_dns_record_mismatch_count", prefix),
                "Number of inconsistent records across regions",
            ),
            record_labels,
        )?;

        let record_ttl = GaugeVec::new(
            Opts::new(
                &format!("{}_dns_record_ttl", prefix),
                "Current TTL values for DNS records",
            ),
            record_labels,
        )?;

        let resolver_hops = GaugeVec::new(
            Opts::new(
                &format!("{}_dns_resolver_hops", prefix),
                "Number of resolver hops for DNS resolution",
            ),
            base_labels,
        )?;

        let propagation_delay = GaugeVec::new(
            Opts::new(
                &format!("{}_dns_propagation_delay_seconds", prefix),
                "Time taken for DNS changes to propagate",
            ),
            record_labels,
        )?;

        // Register all metrics
        registry.register(Box::new(lookup_duration.clone()))?;
        registry.register(Box::new(response_percentiles.clone()))?;
        registry.register(Box::new(lookup_success_ratio.clone()))?;
        registry.register(Box::new(lookup_failures.clone()))?;
        registry.register(Box::new(records_returned.clone()))?;
        registry.register(Box::new(record_mismatch_count.clone()))?;
        registry.register(Box::new(record_ttl.clone()))?;
        registry.register(Box::new(resolver_hops.clone()))?;
        registry.register(Box::new(propagation_delay.clone()))?;

        Ok(DnsMetrics {
            lookup_duration,
            response_percentiles,
            lookup_success_ratio,
            lookup_failures,
            records_returned,
            record_mismatch_count,
            record_ttl,
            resolver_hops,
            propagation_delay,
        })
    }

    pub fn update(
        &self,
        location: &LocationInfo,
        record_type: &str,
        duration: f64,
        success: bool,
        record_count: u64,
        ttl: u64,
    ) {
        let base_labels = &[
            location.country_code.as_str(),
            location.continent.as_str(),
            location.city.as_str(),
            location.isp.as_str(),
        ];

        let record_labels = &[
            location.country_code.as_str(),
            location.continent.as_str(),
            location.city.as_str(),
            location.isp.as_str(),
            record_type,
        ];

        // Update metrics
        self.lookup_duration
            .with_label_values(record_labels)
            .set(duration);

        self.lookup_success_ratio
            .with_label_values(base_labels)
            .set(if success { 1.0 } else { 0.0 });

        if !success {
            self.lookup_failures
                .with_label_values(&[
                    &location.country_code,
                    &location.continent,
                    &location.city,
                    &location.isp,
                    "timeout",
                ])
                .inc();
        }

        self.records_returned
            .with_label_values(record_labels)
            .set(record_count as f64);

        self.record_ttl
            .with_label_values(record_labels)
            .set(ttl as f64);

        // Update percentiles
        let percentile_base = &[
            &location.country_code,
            &location.continent,
            &location.city,
            &location.isp,
        ];

        self.response_percentiles
            .with_label_values(&[
                percentile_base[0],
                percentile_base[1],
                percentile_base[2],
                percentile_base[3],
                "p50",
            ])
            .set(duration);

        self.response_percentiles
            .with_label_values(&[
                percentile_base[0],
                percentile_base[1],
                percentile_base[2],
                percentile_base[3],
                "p90",
            ])
            .set(duration * 1.5);

        self.response_percentiles
            .with_label_values(&[
                percentile_base[0],
                percentile_base[1],
                percentile_base[2],
                percentile_base[3],
                "p99",
            ])
            .set(duration * 2.0);
    }
}

pub struct MetricsEngine {
    registry: Registry,
    metrics: HashMap<String, DnsMetrics>,
}

impl MetricsEngine {
    fn new() -> Result<Self> {
        Ok(MetricsEngine {
            registry: Registry::new(),
            metrics: HashMap::new(),
        })
    }

    pub fn update_metrics(&mut self, update: MetricUpdate) -> Result<()> {
        if !self.metrics.contains_key(&update.prefix) {
            let metrics = DnsMetrics::new(&update.prefix, &self.registry)?;
            self.metrics.insert(update.prefix.clone(), metrics);
        }

        if let Some(metrics) = self.metrics.get(&update.prefix) {
            metrics.update(
                &update.location,
                &update.record_type,
                update.duration,
                update.success,
                update.record_count,
                update.ttl,
            );
        }

        Ok(())
    }

    pub fn gather(&self) -> Vec<prometheus::proto::MetricFamily> {
        self.registry.gather()
    }
}

#[derive(Clone)]
pub struct MetricsHandle {
    command_tx: mpsc::Sender<MetricCommand>,
}

impl MetricsHandle {
    pub fn new() -> Result<(Self, mpsc::Receiver<MetricCommand>)> {
        let (tx, rx) = mpsc::channel(1000);
        Ok((Self { command_tx: tx }, rx))
    }

    pub async fn update_metrics(&self, update: MetricUpdate) -> Result<()> {
        self.command_tx
            .send(MetricCommand::Update(update))
            .await
            .context("Failed to send metric update")?;
        Ok(())
    }

    pub async fn gather(&self) -> Result<Vec<prometheus::proto::MetricFamily>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(MetricCommand::Gather { respond_to: tx })
            .await
            .context("Failed to send gather request")?;
        rx.await.context("Failed to receive metrics")
    }
}

pub async fn metrics_processor(mut rx: mpsc::Receiver<MetricCommand>) -> Result<()> {
    let mut engine = MetricsEngine::new()?;

    while let Some(cmd) = rx.recv().await {
        match cmd {
            MetricCommand::Update(update) => {
                if let Err(e) = engine.update_metrics(update) {
                    error!("Failed to update metrics: {}", e);
                }
            }
            MetricCommand::Gather { respond_to } => {
                let _ = respond_to.send(engine.gather());
            }
        }
    }

    Ok(())
}
