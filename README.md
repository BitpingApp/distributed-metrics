# Distributed Metrics Collector

This tool uses the Bitping Developer API to collect metrics about different protocols and how services respond from an external perspective.

Similar to Uptime testing tools such as BetterUptime or UptimeRobot but you own the data and can hook the data into your own Prometheus & Grafana for reporting.

You can also specify the network type of the reporting device such as if its a Residential IP, a Hosted VPS IP, a Mobile Broadband IP or even behind a Proxy/VPN service.

## Get Started

1. Sign up for the Bitping Developer API at https://developer.bitping.com
2. Generate an API Key
3. Create a Metrics.toml file
4. Set your BITPING_API_KEY environment variable
5. Run `sh ./metric-collector`

### Supported Protocols

- DNS
- ICMP (Coming Soon)
- HTTP GET (Coming Soon)
- HLS (Coming Soon)

#### Example Metrics.toml

```toml
[[metric]]
prefix = "example_com"
type = "Dns"
endpoint = "example.com"
frequency_ms = 60000   # Check every minute

[[metric]]
prefix = "critical_service"
type = "Dns"
endpoint = "bitping.com"
frequency_ms = 10000        # Check every 10 seconds
```

#### Exposed Metrics

##### DNS
