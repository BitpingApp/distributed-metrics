use super::Collector;
use crate::config::{DnsConfig, MetricConfig};
use crate::types::{
    PerformDnsBodyConfiguration, PerformDnsBodyConfigurationLookupTypesItem,
    PerformDnsBodyContinentCode, PerformDnsBodyCountryCode, PerformDnsBodyMobile,
    PerformDnsBodyProxy, PerformDnsBodyResidential, PerformDnsResponse, PerformDnsResponseNodeInfo,
    PerformDnsResponseResultsItemResult,
};
use crate::API_CLIENT;
use color_eyre::eyre::Result;
use metrics::{counter, gauge, histogram};
use progenitor::progenitor_client::Error;
use reqwest::StatusCode;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use tracing::{error, info, warn};

pub struct DnsCollector {
    config: DnsConfig,
}

impl Collector for DnsCollector {
    type Config = DnsConfig;

    fn new(config: DnsConfig) -> Self {
        Self { config }
    }

    fn register_metrics(&self) {
        let prefix = &self.config.common_config.prefix;

        metrics::describe_counter!(
            format!("{}dns_lookup_success_total", prefix),
            "Total number of successful DNS lookups"
        );

        metrics::describe_counter!(
            format!("{}dns_lookup_failures_total", prefix),
            "Total number of DNS lookup failures"
        );

        metrics::describe_gauge!(
            format!("{}dns_lookup_success_ratio", prefix),
            "Ratio of successful DNS lookups to total lookups"
        );

        metrics::describe_histogram!(
            format!("{}dns_server_lookup_duration_ms", prefix),
            "Time taken to perform DNS lookup in ms"
        );

        metrics::describe_gauge!(
            format!("{}dns_ip_records_count", prefix),
            "Number of IP address records (A/AAAA) in DNS response"
        );

        metrics::describe_gauge!(
            format!("{}dns_mx_records_count", prefix),
            "Number of mail exchanger (MX) records in DNS response"
        );

        metrics::describe_gauge!(
            format!("{}dns_txt_records_count", prefix),
            "Number of text (TXT) records in DNS response"
        );

        metrics::describe_gauge!(
            format!("{}dns_ns_records_count", prefix),
            "Number of nameserver (NS) records in DNS response"
        );

        metrics::describe_gauge!(
            format!("{}dns_srv_records_count", prefix),
            "Number of service (SRV) records in DNS response"
        );

        metrics::describe_gauge!(
            format!("{}dns_tlsa_records_count", prefix),
            "Number of TLSA records in DNS response"
        );

        metrics::describe_gauge!(
            format!("{}dns_soa_records_count", prefix),
            "Number of SOA records in DNS response"
        );
    }

    async fn perform_request(&self) -> Result<()> {
        let country_code = self
            .config
            .common_config
            .network
            .as_ref()
            .and_then(|x| x.country_code)
            .map(|c| c.to_alpha2().to_string())
            .and_then(|x| PerformDnsBodyCountryCode::from_str(&x).ok());

        let continent_code = self
            .config
            .common_config
            .network
            .as_ref()
            .and_then(|x| x.continent_code.clone())
            .and_then(|c| PerformDnsBodyContinentCode::from_str(c.as_ref()).ok());

        let mobile = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.mobile.as_ref().to_uppercase())
            .and_then(|mo| PerformDnsBodyMobile::from_str(&mo).ok())
            .unwrap_or_default();

        let residential = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.residential.as_ref().to_uppercase())
            .and_then(|mo| PerformDnsBodyResidential::from_str(&mo).ok())
            .unwrap_or_default();

        let proxy = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.proxy.as_ref().to_uppercase())
            .and_then(|mo| PerformDnsBodyProxy::from_str(&mo).ok())
            .unwrap_or_default();

        info!(?self.config.common_config, ?country_code, "Sending DNS request");

        match API_CLIENT
            .perform_dns()
            .body_map(|body| {
                body.hostnames([self.config.common_config.endpoint.clone()])
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
                self.handle_response(response.into_inner());
                Ok(())
            }
            Err(e) => {
                warn!(?e, "API Error");
                Ok(())
            }
        }
    }

    fn get_frequency(&self) -> std::time::Duration {
        self.config.common_config.frequency
    }
}

impl DnsCollector {
    fn handle_response(&self, response: PerformDnsResponse) {
        if let Some(result) = response.results.first() {
            if let (Some(dns_result), Some(node_info)) = (result.result.clone(), response.node_info)
            {
                let prefix = &self.config.common_config.prefix;

                // Core labels - essential dimensions only
                let common_labels = [
                    ("country_code", node_info.country_code.clone()),
                    ("continent", node_info.continent_code.clone()),
                    ("city", node_info.city.clone()),
                    ("isp", node_info.isp.clone()),
                    // ("os", node_info.operating_system.clone()),
                    // ("residential", node_info.residential.to_string()),
                    // ("proxy", node_info.proxy.to_string()),
                    // ("mobile", node_info.mobile.to_string()),
                    ("endpoint", result.endpoint.clone()),
                ];

                let cleaned_dns_ips = dns_result
                    .dns_servers
                    .iter()
                    .map(|s| s.replace("udp:", "").replace("tcp:", "").replace(":53", ""));
                let dns_providers = identify_dns_providers(cleaned_dns_ips);

                for server in dns_providers {
                    let mut server_labels = common_labels.to_vec();
                    server_labels.push(("dns_server", server));

                    // Core metrics
                    histogram!(
                        format!("{}dns_server_lookup_duration_ms", prefix),
                        &server_labels
                    )
                    .record(result.duration.unwrap_or(0.0));

                    // Record counts and check for success/failure
                    let record_counts = [
                        (
                            "ip",
                            dns_result.ips.len(),
                            Self::hash_records(&dns_result.ips[..]),
                        ),
                        (
                            "mx",
                            dns_result.mx.len(),
                            Self::hash_records(&dns_result.mx[..]),
                        ),
                        (
                            "txt",
                            dns_result.txt.len(),
                            Self::hash_records(&dns_result.txt[..]),
                        ),
                        (
                            "ns",
                            dns_result.ns.len(),
                            Self::hash_records(&dns_result.ns[..]),
                        ),
                        (
                            "srv",
                            dns_result.srv.len(),
                            Self::hash_records(&dns_result.srv[..]),
                        ),
                        (
                            "tlsa",
                            dns_result.tlsa.len(),
                            Self::hash_records(&dns_result.tlsa[..]),
                        ),
                        (
                            "soa",
                            dns_result.soa.len(),
                            Self::hash_records(&dns_result.soa[..]),
                        ),
                    ];

                    // Check if all record types have at least one entry
                    let has_all_records = record_counts.iter().all(|(_, count, _)| *count > 0);

                    if has_all_records {
                        counter!(
                            format!("{}dns_lookup_success_total", prefix),
                            &server_labels
                        )
                        .increment(1);
                    } else {
                        counter!(
                            format!("{}dns_lookup_failures_total", prefix),
                            &server_labels
                        )
                        .increment(1);
                    }

                    // Calculate success ratio
                    gauge!(
                        format!("{}dns_lookup_success_ratio", prefix),
                        &server_labels
                    )
                    .set(if has_all_records { 1.0 } else { 0.0 });

                    // Always record the count metrics regardless of success/failure
                    for (record_type, count, hash) in record_counts {
                        let mut record_labels: Vec<(String, String)> = server_labels
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .collect();
                        record_labels
                            .push((format!("{record_type}_record_hash"), hash.to_string()));

                        gauge!(
                            format!("{}dns_{}_records_count", prefix, record_type),
                            &record_labels
                        )
                        .set(count as f64);
                    }
                }
            }
        }
    }

    fn hash_records<T: AsRef<str>>(records: &[T]) -> u64 {
        use std::collections::BTreeSet;

        let normalized: BTreeSet<String> =
            records.iter().map(|s| s.as_ref().to_lowercase()).collect();

        let mut hasher = DefaultHasher::new();
        normalized.hash(&mut hasher);
        hasher.finish()
    }

    fn record_failure(&self, error_type: &str) {}
}

fn identify_dns_providers<I>(ips: I) -> HashSet<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut providers = HashSet::new();
    let provider_map: HashMap<&str, &str> = [
        ("8.8.8.8", "Google"),
        ("8.8.4.4", "Google"),
        ("1.1.1.1", "Cloudflare"),
        ("1.0.0.1", "Cloudflare"),
        ("9.9.9.9", "Quad9"),
        ("149.112.112.112", "Quad9"),
        ("208.67.222.222", "OpenDNS"),
        ("208.67.220.220", "OpenDNS"),
        ("94.140.14.14", "AdGuard"),
        ("94.140.15.15", "AdGuard"),
        ("185.228.168.9", "CleanBrowsing"),
        ("185.228.169.9", "CleanBrowsing"),
        ("76.76.19.19", "Alternate DNS"),
        ("76.223.122.150", "Alternate DNS"),
        ("76.76.2.0", "Control D"),
        ("76.76.10.0", "Control D"),
    ]
    .iter()
    .cloned()
    .collect();

    for ip in ips {
        let provider = provider_map
            .get(ip.as_ref())
            .map(|&p| p.to_string())
            .unwrap_or_else(|| ip.as_ref().to_string());
        providers.insert(provider);
    }

    providers
}
