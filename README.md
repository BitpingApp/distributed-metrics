# Distributed Metrics Collector

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
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/BitpingApp/distributed-metrics/releases/latest/download/dns-metric-collector-installer.sh | sh
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
  bitpingapp/distributed-metrics
```

### Run Docker Compose

Save the following yaml to docker-compose.yaml
```yaml
version: '3'
services:
  metrics:
    image: bitpingapp/distributed-metrics
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
    endpoint: example.com
    frequency_ms: 1000
    network:
      proxy: denied
      mobile: allowed
      residential: required
      country_code: NLD # Optional: ISO 3166-1 alpha-3 country code
      continent_code: EU # Optional: AF, AN, AS, EU, NA, OC, SA
```

Metrics collected:

- `dns_lookup_duration_seconds`: Time taken for DNS resolution
- `dns_lookup_success_ratio`: Success rate of DNS lookups
- `dns_records_returned`: Number of DNS records returned
- `dns_lookup_failures_total`: Count of DNS lookup failures

Labels:

- country_code
- continent
- city
- isp
- endpoint
- record_type (for records_returned)

### ICMP

Measures network latency and packet loss.

```yaml
metrics:
  - type: icmp
    endpoint: example.com
    frequency_ms: 1000
    network:
      proxy: denied
      mobile: allowed
      residential: required
```

Metrics collected:

- `icmp_ping_duration_seconds`: Overall ping duration
- `icmp_ping_latency_min_ms`: Minimum latency
- `icmp_ping_latency_max_ms`: Maximum latency
- `icmp_ping_latency_avg_ms`: Average latency
- `icmp_ping_latency_stddev_ms`: Standard deviation of latency
- `icmp_ping_packet_loss_ratio`: Ratio of lost packets
- `icmp_ping_packets_sent`: Number of packets sent
- `icmp_ping_packets_received`: Number of packets received
- `icmp_ping_success_ratio`: Success rate of pings
- `icmp_ping_failures_total`: Count of ping failures

Labels:

- country_code
- continent
- city
- isp
- endpoint
- ip_address

### HLS

Measures HLS video stream performance and quality metrics.

```yaml
metrics:
  - type: hls
    endpoint: https://example.com/stream.m3u8
    frequency_ms: 15000
```

Metrics collected:

- `hls_master_download_duration_seconds`: Time to download master playlist
- `hls_master_size_bytes`: Size of master playlist
- `hls_renditions_count`: Number of available quality levels
- `hls_tcp_connect_duration_seconds`: TCP connection time
- `hls_ttfb_duration_seconds`: Time to first byte
- `hls_dns_resolve_duration_seconds`: DNS resolution time
- `hls_tls_handshake_duration_seconds`: TLS handshake time
- `hls_fragment_download_duration_seconds`: Fragment download time
- `hls_fragment_size_bytes`: Fragment sizes
- `hls_fragment_bandwidth_bytes_per_second`: Fragment download speeds
- `hls_fragment_duration_seconds`: Fragment durations
- `hls_rendition_bandwidth_bits_per_second`: Rendition bandwidths
- `hls_failures_total`: Count of HLS failures

Labels:

- country_code
- continent
- city
- isp
- endpoint
- resolution (for rendition metrics)
- bandwidth (for rendition metrics)

## Configuration

### Network Selection Parameters

All protocols support these network selection criteria:

- `proxy`: Policy for proxy nodes (allowed, denied, required)
- `mobile`: Policy for mobile nodes (allowed, denied, required)
- `residential`: Policy for residential nodes (allowed, denied, required)
- `continent_code`: Optional continent restriction (AF, AN, AS, EU, NA, OC, SA)
- `country_code`: Optional country restriction (ISO 3166-1 alpha-3)

### Example Full Configuration

```yaml
metrics:
  - type: dns
    endpoint: example.com
    frequency_ms: 1000
    network:
      proxy: denied
      mobile: allowed
      residential: required
      country_code: NLD
      continent_code: EU

  - type: icmp
    endpoint: example.com
    frequency_ms: 1000
    network:
      proxy: denied
      mobile: allowed
      residential: required

  - type: hls
    endpoint: https://example.com/stream.m3u8
    frequency_ms: 15000
```

## Error Handling

All collectors track failures with specific error types:

- `no_nodes_found`: No nodes matching the network criteria
- `api_error`: API communication errors
- `missing_data`: Incomplete/invalid response data
- `no_results`: Empty response from API

These errors are tracked in the `*_failures_total` metrics with an `error_type` label.

## Coming Soon

- HTTP GET metrics
- Additional protocol support
- More detailed documentation and examples
