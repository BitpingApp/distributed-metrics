use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsErrorType {
    NoRecord(DnsRecordType),
    NetworkError(String),
    ConfigurationError(String),
    Timeout(String),
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DnsRecordType {
    A,
    AAAA,
    MX,
    TXT,
    NS,
    SOA,
    SRV,
    TLSA,
    Unknown,
}

impl std::fmt::Display for DnsRecordType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DnsRecordType::A => write!(f, "A"),
            DnsRecordType::AAAA => write!(f, "AAAA"),
            DnsRecordType::MX => write!(f, "MX"),
            DnsRecordType::TXT => write!(f, "TXT"),
            DnsRecordType::NS => write!(f, "NS"),
            DnsRecordType::SOA => write!(f, "SOA"),
            DnsRecordType::SRV => write!(f, "SRV"),
            DnsRecordType::TLSA => write!(f, "TLSA"),
            DnsRecordType::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

pub struct DnsErrorParser;

impl DnsErrorParser {
    pub fn parse(error_message: &str) -> Vec<DnsErrorType> {
        let mut errors = Vec::new();
        let mut seen_record_types = HashSet::new();

        for line in error_message.lines() {
            let trimmed = line.trim();

            if trimmed.is_empty() {
                continue;
            }

            if let Some(error) = Self::parse_no_record_error(trimmed) {
                if seen_record_types.insert(error) {
                    errors.push(DnsErrorType::NoRecord(error));
                }
                continue;
            }

            if let Some(error) = Self::parse_network_error(trimmed) {
                errors.push(DnsErrorType::NetworkError(error));
                continue;
            }

            if let Some(error) = Self::parse_timeout_error(trimmed) {
                errors.push(DnsErrorType::Timeout(error));
                continue;
            }

            errors.push(DnsErrorType::Other(trimmed.to_string()));
        }

        errors
    }

    fn parse_no_record_error(line: &str) -> Option<DnsRecordType> {
        if line.contains("no record found for Query") {
            let record_type = if line.contains("query_type: A,") {
                DnsRecordType::A
            } else if line.contains("query_type: AAAA,") {
                DnsRecordType::AAAA
            } else if line.contains("query_type: MX,") {
                DnsRecordType::MX
            } else if line.contains("query_type: TXT,") {
                DnsRecordType::TXT
            } else if line.contains("query_type: NS,") {
                DnsRecordType::NS
            } else if line.contains("query_type: SOA,") {
                DnsRecordType::SOA
            } else if line.contains("query_type: SRV,") {
                DnsRecordType::SRV
            } else if line.contains("query_type: TLSA,") {
                DnsRecordType::TLSA
            } else {
                DnsRecordType::Unknown
            };
            Some(record_type)
        } else {
            None
        }
    }

    fn parse_network_error(line: &str) -> Option<String> {
        if line.contains("network error") || line.contains("connection refused") {
            Some(line.to_string())
        } else {
            None
        }
    }

    fn parse_timeout_error(line: &str) -> Option<String> {
        if line.contains("timed out") {
            Some(line.to_string())
        } else {
            None
        }
    }
}
