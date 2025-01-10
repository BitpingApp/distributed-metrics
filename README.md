# Global Metrics Collector

This tool uses the Bitping Developer API to collect metrics about different protocols and how services respond from an external perspective.

Similar to Uptime testing tools such as BetterUptime or UptimeRobot but you own the data and can hook the data into your own Prometheus & Grafana for reporting.

You can also specify the network type of the reporting device such as if its a Residential IP, a Hosted VPS IP, a Mobile Broadband IP or even behind a Proxy/VPN service.

## Get Started

1. Sign up for the Bitping Developer API at https://developer.bitping.com
2. Generate an API Key
3. Create a `Metrics.yaml` file (see configuration below)
4. Set your BITPING_API_KEY environment variable:
   ```bash
   export BITPING_API_KEY=your_api_key
   ```
5. Follow the install instructions below
6. Run:
   ```bash
   distributed-metrics
   ```
Metrics will be available at `http://localhost:3000/metrics` in Prometheus format.

## Installation

### Install prebuilt binaries via shell script

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/BitpingApp/distributed-metrics/releases/latest/download/distributed-metrics-installer.sh | sh
```

### Install prebuilt binaries via Homebrew

```sh
brew install BitpingApp/tap/distributed-metrics
```

### Run Docker Container

```bash
docker run -d \
  -p 3000:3000 \
  -e BITPING_API_KEY=your_api_key \
  -v $(pwd)/Metrics.yaml:/app/Metrics.yaml \
  bitping/distributed-metrics
```

### Run Docker Compose

Save the following yaml to docker-compose.yaml
```yaml
version: '3'
services:
  metrics:
    image: bitping/distributed-metrics
    ports:
      - "3000:3000"
    environment:
      - BITPING_API_KEY=your_api_key
    volumes:
      - ./Metrics.yaml:/app/Metrics.yaml
    restart: unless-stopped
```

Run:
```bash
docker compose up -d
```

## Supported Protocols

### DNS

Measures DNS resolution performance and reliability.

```yaml
metrics:
  - type: dns
    prefix: "custom_prefix_" # Optional prefix for metrics
    name: "custom_name" # Optional name override
    endpoint: example.com
    frequency: 1s
    network:
      proxy: denied
      mobile: allowed
      residential: required
      country_code: NLD # Optional: ISO 3166-1 alpha-3 country code
      continent_code: EU # Optional: AF, AN, AS, EU, NA, OC, SA
      isp_regex: "^Comcast" # Optional: Filter by ISP name
      node_id: "node123" # Optional: Specific node ID
    lookup_type: IP # Optional: IP, MX, SOA, NS, TXT, SRV, TLSA (default: IP)
```

Metrics collected:

- `dns_lookup_success_total`: Count of successful DNS lookups
- `dns_lookup_error_total`: Count of DNS lookup errors by type
- `dns_lookup_total`: Total number of DNS lookups attempted
- `dns_server_lookup_duration_ms`: Time taken for DNS resolution
- `dns_record_hash`: Hash of the DNS response for change detection
- `dns_records_count`: Number of records returned
- `dns_soa_records_count`: Number of SOA records (when applicable)

Labels:
- country_code
- continent
- city
- isp
- os
- endpoint
- dns_server
- record_type
- error_type (for errors)

### ICMP

Measures network latency and packet loss.

```yaml
metrics:
  - type: icmp
    prefix: "custom_prefix_" # Optional prefix for metrics
    name: "custom_name" # Optional name override
    endpoint: example.com
    frequency: 1s
    network:
      proxy: denied
      mobile: allowed
      residential: required
```

Metrics collected:

- `icmp_ping_failures_total`: Count of ping failures
- `icmp_ping_success_total`: Count of successful pings
- `icmp_ping_duration_ms`: Overall ping duration
- `icmp_ping_latency_min_ms`: Minimum latency
- `icmp_ping_latency_max_ms`: Maximum latency
- `icmp_ping_latency_avg_ms`: Average latency
- `icmp_ping_latency_stddev_ms`: Standard deviation of latency
- `icmp_ping_packet_loss_ratio`: Ratio of lost packets
- `icmp_ping_packets_sent`: Number of packets sent
- `icmp_ping_packets_received`: Number of packets received
- `icmp_ping_success_ratio`: Success rate of pings

Labels:
- country_code
- continent
- city
- isp
- os
- endpoint
- ip_address
- error_type (for failures)

### HLS

Measures HLS video stream performance and quality metrics.

```yaml
metrics:
  - type: hls
    prefix: "custom_prefix_" # Optional prefix for metrics
    name: "custom_name" # Optional name override
    endpoint: https://example.com/stream.m3u8
    frequency: 15s
    network:
      proxy: denied
      mobile: allowed
      residential: required
    headers: # Optional custom headers
      User-Agent: "CustomPlayer/1.0"
      Authorization: "Bearer token123"
```

Metrics collected:

- `hls_total_ms`: Total time taken for HLS test
- `hls_master_download_ms`: Master playlist download time
- `hls_master_size_bytes`: Master playlist size
- `hls_master_bitrate`: Master playlist download speed
- `hls_renditions_count`: Number of available renditions
- `hls_master_tcp_connect_ms`: TCP connection time
- `hls_master_ttfb_ms`: Time to first byte
- `hls_master_dns_resolve_ms`: DNS resolution time
- `hls_master_tls_handshake_ms`: TLS handshake time
- `hls_fragment_download_ms`: Fragment download times
- `hls_fragment_size_bytes`: Fragment sizes
- `hls_fragment_bandwidth_bytes_per`: Fragment bandwidth
- `hls_fragment_duration_seconds`: Fragment durations
- `hls_buffer_fill_rate`: Buffer fill rate vs playback speed
- `hls_estimated_buffer_ms`: Estimated buffer length
- `hls_initial_buffer_ms`: Initial buffering time
- `hls_playlist_chain_load_time`: Total playlist load time
- `hls_failures_total`: Count of failures
- `hls_errors_by_type`: Errors by category

Labels:
- country_code
- continent
- city
- isp
- os
- endpoint
- resolution
- bandwidth
- target_duration_secs
- discontinuity_sequence
- playlist_type
- error_type (for failures)

## Configuration

### Global Configuration

```yaml
metric_clear_timeout: 10s # How long to keep metrics after a scrape has occured - prevents timeouts on scraping as cardinality can be high

metrics:
  # Protocol configurations as shown above
```

### Network Selection Parameters

All protocols support these network selection criteria:

- `proxy`: Policy for proxy nodes (allowed, denied, required)
- `mobile`: Policy for mobile nodes (allowed, denied, required)
- `residential`: Policy for residential nodes (allowed, denied, required)
- `continent_code`: Optional continent restriction (AF, AN, AS, EU, NA, OC, SA)
- `country_code`: Optional country restriction (ISO 3166-1 alpha-3)
- `isp_regex`: Optional ISP name filter using regex
- `node_id`: Optional specific node selection

### Common Metric Configuration

All metrics support these base configuration options:

- `prefix`: Optional prefix for metric names
- `name`: Optional name override for the endpoint label
- `endpoint`: Target hostname or URL
- `frequency`: How often to collect metrics (e.g., "1s", "15s", "1m")
- `network`: Network selection criteria (see above)

## Error Handling

All collectors track failures with specific error types in their respective `*_failures_total` or `*_errors_by_type` metrics. Common error categories include:

- DNS: no_records, connection_refused, timeout, resolution_failed, server_misbehaving
- ICMP: dns_lookup_failed, timeout, host_unreachable, permission_denied, network_unreachable
- HLS: dns_error, not_found, invalid_manifest, timeout, connection_error, ssl_error, http_4xx/5xx
