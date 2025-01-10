mod errors;

use super::{Collector, CollectorErrors};
use crate::config::{DnsConfig, LookupTypes, MetricConfig};
use crate::types::{
    PerformDnsBodyConfiguration, PerformDnsBodyConfigurationLookupTypesItem,
    PerformDnsBodyContinentCode, PerformDnsBodyCountryCode, PerformDnsBodyMobile,
    PerformDnsBodyProxy, PerformDnsBodyResidential, PerformDnsResponse, PerformDnsResponseNodeInfo,
    PerformDnsResponseResultsItemResult, PerformHlsResponse,
};
use crate::API_CLIENT;
use color_eyre::eyre::Result;
use metrics::{counter, gauge, histogram};
use progenitor::progenitor_client::Error;
use reqwest::StatusCode;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Deref;
use std::str::FromStr;
use tracing::{error, info, instrument, warn};

pub struct DnsCollector {
    config: &'static DnsConfig,
}

impl Collector for DnsCollector {
    type Config = DnsConfig;
    type Response = PerformDnsResponse;

    fn new(config: &'static DnsConfig) -> Self {
        Self { config }
    }

    fn register_metrics(&self) {
        let prefix = &self.config.common_config.prefix;

        metrics::describe_counter!(
            format!("{}dns_lookup_success_total", prefix),
            "Total number of successful DNS lookups"
        );

        metrics::describe_counter!(
            format!("{}dns_lookup_error_total", self.config.common_config.prefix),
            "Total number of DNS lookup errors by type"
        );

        metrics::describe_counter!(format!("{}dns_lookup_total", prefix), "Total DNS lookups");

        metrics::describe_histogram!(
            format!("{}dns_server_lookup_duration_ms", prefix),
            "Time taken to perform DNS lookup in ms"
        );

        metrics::describe_gauge!(
            format!("{}dns_record_hash", prefix),
            "Records the hash of the response the node saw"
        );

        metrics::describe_gauge!(
            format!("{}dns_records_count", prefix),
            "Number of records in DNS response"
        );

        metrics::describe_gauge!(
            format!("{}dns_soa_records_count", prefix),
            "Number of SOA records in DNS response"
        );
    }

    async fn perform_request(&self) -> Result<Self::Response> {
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

        let isp = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.isp_regex.clone())
            .unwrap_or_default();

        let node_id = self
            .config
            .common_config
            .network
            .as_ref()
            .map(|n| n.node_id.clone())
            .unwrap_or_default();

        info!(?self.config.common_config, ?country_code, "Sending DNS request");

        let response = API_CLIENT
            .perform_dns()
            .body_map(|body| {
                body.hostnames([self.config.common_config.endpoint.clone()])
                    .country_code(country_code)
                    .continent_code(continent_code)
                    .mobile(mobile)
                    .residential(residential)
                    .isp_regex(isp)
                    .node_id(node_id)
                    .proxy(proxy)
                    .configuration(Some(PerformDnsBodyConfiguration {
                        dns_servers: vec![],
                        lookup_types: vec![PerformDnsBodyConfigurationLookupTypesItem::from_str(
                            self.config.lookup_type.as_ref(),
                        )
                        .unwrap()],
                    }))
            })
            .send()
            .await?;

        Ok(response.into_inner())
    }

    fn get_frequency(&self) -> std::time::Duration {
        self.config.common_config.frequency
    }

    fn handle_response(&self, response: PerformDnsResponse) -> Result<(), CollectorErrors> {
        let endpoint = self
            .config
            .common_config
            .name
            .as_ref()
            .unwrap_or(&self.config.common_config.endpoint);

        let node_info = response
            .node_info
            .ok_or_else(|| CollectorErrors::MissingNodeInfo(endpoint.clone()))?;

        // Core labels - essential dimensions only
        let mut labels: HashMap<&str, String> = HashMap::from_iter([
            ("country_code", node_info.country_code.clone()),
            ("continent", node_info.continent_code.clone()),
            ("city", node_info.city.clone()),
            ("isp", node_info.isp.clone()),
            ("os", node_info.operating_system.clone()),
            ("endpoint", endpoint.clone()),
        ]);

        if let Some(result) = response.results.first() {
            if let Some(error) = &result.error {
                // Handle error case
                self.record_failure_with_labels(error, &labels);
            } else if let Some(dns_result) = &result.result {
                // Handle success case
                let cleaned_dns_ips = dns_result
                    .dns_servers
                    .iter()
                    .map(|s| s.replace("udp:", "").replace("tcp:", "").replace(":53", ""));

                let dns_providers = identify_dns_providers(cleaned_dns_ips);

                for server in dns_providers {
                    labels.insert("dns_server", server);
                    self.record_success_metrics(
                        dns_result,
                        result.duration.unwrap_or(0.0),
                        &labels,
                    );
                }
            } else {
                error!("Missing DNS result data");
                return Err(CollectorErrors::MissingData(endpoint.clone(), "dns_result"));
            }
        } else {
            error!("No results returned from API");
            return Err(CollectorErrors::MissingData(endpoint.clone(), "no_results"));
        }

        Ok(())
    }
}

impl DnsCollector {
    fn record_failure_with_labels(&self, error: &str, labels: &HashMap<&'static str, String>) {
        let mut labels = labels.clone();
        let error_type = match error {
            e if e.contains("no record found for Query") => "no_records",
            e if e.contains("connection refused") => "connection_refused",
            e if e.contains("connection timed out") => "timeout",
            e if e.contains("name resolution failed") => "resolution_failed",
            e if e.contains("server misbehaving") => "server_misbehaving",
            e if e.contains("network is unreachable") => "network_unreachable",
            e => {
                warn!(?e, "Unable to parse DNS error, returning unknown_error");
                "unknown_error"
            }
        };
        labels.insert("error_type", error_type.into());

        counter!(
            format!("{}dns_lookup_error_total", self.config.common_config.prefix),
            &labels
        )
        .increment(1);
    }

    fn record_success_metrics(
        &self,
        result: &PerformDnsResponseResultsItemResult,
        duration: f64,
        labels: &HashMap<&'static str, String>,
    ) {
        let prefix = &self.config.common_config.prefix;

        // Record lookup duration
        histogram!(format!("{}dns_server_lookup_duration_ms", prefix), labels).record(duration);

        // Record counts and hashes based on lookup type
        let (record_type, count, hash) = match self.config.lookup_type {
            LookupTypes::IP => ("ip", result.ips.len(), Self::hash_records(&result.ips[..])),
            LookupTypes::MX => ("mx", result.mx.len(), Self::hash_records(&result.mx[..])),
            LookupTypes::TXT => ("txt", result.txt.len(), Self::hash_records(&result.txt[..])),
            LookupTypes::NS => ("ns", result.ns.len(), Self::hash_records(&result.ns[..])),
            LookupTypes::SRV => ("srv", result.srv.len(), Self::hash_records(&result.srv[..])),
            LookupTypes::TLSA => (
                "tlsa",
                result.tlsa.len(),
                Self::hash_records(&result.tlsa[..]),
            ),
            LookupTypes::SOA => ("soa", result.soa.len(), Self::hash_records(&result.soa[..])),
        };

        let mut record_labels = labels.clone();
        record_labels.insert("record_type", record_type.into());

        gauge!(format!("{}dns_record_hash", prefix), &record_labels).set(hash as f64);
        gauge!(format!("{}dns_records_count", prefix), &record_labels).set(count as f64);

        if count > 0 {
            counter!(
                format!("{}dns_lookup_success_total", prefix),
                &record_labels
            )
            .increment(1);
        }

        counter!(format!("{}dns_lookup_total", prefix), &record_labels).increment(1);
    }

    fn hash_records<T: AsRef<str>>(records: &[T]) -> u64 {
        use std::collections::BTreeSet;

        let normalized: BTreeSet<String> =
            records.iter().map(|s| s.as_ref().to_lowercase()).collect();

        let mut hasher = DefaultHasher::new();
        normalized.hash(&mut hasher);
        hasher.finish()
    }
}

fn identify_dns_providers<I>(ips: I) -> HashSet<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut providers = HashSet::new();
    let provider_map = build_provider_map();

    for ip in ips {
        let ip_str = ip.as_ref();

        let ip_addr = match IpAddr::from_str(ip_str) {
            Ok(addr) => addr,
            Err(e) => {
                warn!(?e, ?ip_str, "Got an invalid IP Address for DNS Provider");
                // providers.insert("Invalid IP".to_string());
                continue;
            }
        };

        let provider = match ip_addr {
            IpAddr::V4(ipv4) => {
                if is_loopback_v4(&ipv4) {
                    "Localhost".to_string()
                } else if is_private_ipv4(&ipv4) {
                    classify_private_network(&ipv4)
                } else if is_tailscale_range(&ipv4) {
                    "Tailscale".to_string()
                } else if let Some(provider) = provider_map.get(ip_str) {
                    provider.to_string()
                } else {
                    classify_public_dns(&ipv4)
                }
            }
            IpAddr::V6(ipv6) => {
                if is_loopback_v6(&ipv6) {
                    "Localhost".to_string()
                } else if is_private_ipv6(&ipv6) {
                    classify_private_ipv6(&ipv6)
                } else if let Some(provider) = provider_map.get(ip_str) {
                    provider.to_string()
                } else {
                    classify_public_ipv6_dns(&ipv6)
                }
            }
        };

        providers.insert(provider);
    }

    providers
}

fn is_loopback_v4(ip: &Ipv4Addr) -> bool {
    ip.octets()[0] == 127
}

fn is_loopback_v6(ip: &Ipv6Addr) -> bool {
    *ip == Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)
}

fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();
    match octets[0] {
        10 => true,
        172 => (16..=31).contains(&octets[1]),
        192 => octets[1] == 168,
        169 => octets[1] == 254,
        _ => false,
    }
}

fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    let segments = ip.segments();

    // fc00::/7 (unique local)
    (segments[0] & 0xfe00) == 0xfc00 ||
    // fe80::/10 (link local)
    (segments[0] & 0xffc0) == 0xfe80
}

fn is_tailscale_range(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn build_provider_map() -> HashMap<String, String> {
    let mut map = HashMap::new();

    // Google DNS
    for ip in [
        "8.8.8.8",
        "8.8.4.4",
        "2001:4860:4860::8888",
        "2001:4860:4860::8844",
    ]
    .iter()
    {
        map.insert((*ip).to_string(), "Google".to_string());
    }

    // Cloudflare DNS
    for ip in [
        "1.1.1.1",
        "1.0.0.1",
        "1.1.1.2",
        "1.0.0.2",
        "2606:4700:4700::1111",
        "2606:4700:4700::1001",
        "2606:4700:4700::1112",
        "2606:4700:4700::1002",
    ]
    .iter()
    {
        map.insert((*ip).to_string(), "Cloudflare".to_string());
    }

    // Quad9
    for ip in ["9.9.9.9", "149.112.112.112", "2620:fe::fe", "2620:fe::9"].iter() {
        map.insert((*ip).to_string(), "Quad9".to_string());
    }

    // OpenDNS
    for ip in [
        "208.67.222.222",
        "208.67.220.220",
        "2620:119:35::35",
        "2620:119:53::53",
    ]
    .iter()
    {
        map.insert((*ip).to_string(), "OpenDNS".to_string());
    }

    // AdGuard
    for ip in [
        "94.140.14.14",
        "94.140.15.15",
        "2a10:50c0::ad1:ff",
        "2a10:50c0::ad2:ff",
    ]
    .iter()
    {
        map.insert((*ip).to_string(), "AdGuard".to_string());
    }

    // NextDNS (including all Anycast ranges)
    let nextdns_ranges = (0..=255)
        .filter(|&n| {
            matches!(
                n,
                0 | 1 | 11 | 42 | 68 | 99 | 139 | 165 | 185 | 216 | 233 | 241
            )
        })
        .flat_map(|n| {
            vec![
                format!("45.90.28.{}", n),
                format!("45.90.30.{}", n),
                format!("2a07:a8c0::{:x}", n),
                format!("2a07:a8c1::{:x}", n),
            ]
        });

    for ip in nextdns_ranges {
        map.insert(ip, "NextDNS".to_string());
    }

    map
}

fn classify_private_network(ip: &Ipv4Addr) -> String {
    let octets = ip.octets();
    match octets[0] {
        10 => "Private Network (10.0.0.0/8)".to_string(),
        172 if (16..=31).contains(&octets[1]) => "Private Network (172.16.0.0/12)".to_string(),
        192 if octets[1] == 168 => "Home Network (192.168.0.0/16)".to_string(),
        169 if octets[1] == 254 => "Link-local Network".to_string(),
        _ => "Unknown Private Network".to_string(),
    }
}

fn classify_private_ipv6(ip: &Ipv6Addr) -> String {
    let segments = ip.segments();
    if (segments[0] & 0xfe00) == 0xfc00 {
        "Unique Local Address".to_string()
    } else if (segments[0] & 0xffc0) == 0xfe80 {
        "Link-local Network".to_string()
    } else {
        "Private IPv6 Network".to_string()
    }
}

fn classify_public_dns(ip: &Ipv4Addr) -> String {
    let octets = ip.octets();

    match octets[0] {
        // Reserved for IANA special use
        0 | 127 | 169 | 192 | 198 => "Reserved Range".to_string(),

        // Major ISP ranges
        1..=100 => {
            if is_known_isp_range(ip) {
                "ISP DNS".to_string()
            } else {
                "Unknown Public DNS".to_string()
            }
        }

        // Enterprise ranges
        128..=191 => "Enterprise DNS".to_string(),

        _ => "Unknown Public DNS".to_string(),
    }
}

fn classify_public_ipv6_dns(ip: &Ipv6Addr) -> String {
    let segments = ip.segments();

    match segments[0] {
        // Global Unicast Address (2000::/3)
        s if (0x2000..=0x3FFF).contains(&s) => {
            if is_known_ipv6_dns_range(ip) {
                "Known IPv6 DNS Provider".to_string()
            } else {
                "ISP IPv6 DNS".to_string()
            }
        }

        _ => "Unknown IPv6 DNS".to_string(),
    }
}

fn is_known_isp_range(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();

    matches!(
        (octets[0], octets[1]),
        (24..=50, _) |    // ARIN space
        (62..=70, _) |    // RIPE space
        (80..=90, _) |    // RIPE space
        (98..=100, _) // APNIC space
    )
}

fn is_known_ipv6_dns_range(ip: &Ipv6Addr) -> bool {
    let segments = ip.segments();

    matches!(
        segments[0],
        0x2001 |  // Teredo
        0x2606 |  // Various providers
        0x2620 // Various providers
    )
}
