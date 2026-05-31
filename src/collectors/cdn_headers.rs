use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdnProvider {
    Cloudflare,
    Fastly,
    Cloudfront,
    Akamai,
    Vercel,
    Bunny,
    Gcore,
    Netlify,
    None,
}

impl CdnProvider {
    pub fn as_label(self) -> &'static str {
        match self {
            CdnProvider::Cloudflare => "cloudflare",
            CdnProvider::Fastly => "fastly",
            CdnProvider::Cloudfront => "cloudfront",
            CdnProvider::Akamai => "akamai",
            CdnProvider::Vercel => "vercel",
            CdnProvider::Bunny => "bunny",
            CdnProvider::Gcore => "gcore",
            CdnProvider::Netlify => "netlify",
            CdnProvider::None => "none",
        }
    }
}

pub fn parse_cdn_provider(headers: &HashMap<String, String>) -> CdnProvider {
    if get_header(headers, "cf-ray").is_some() || server_contains(headers, "cloudflare") {
        return CdnProvider::Cloudflare;
    }

    if get_header(headers, "x-amz-cf-id").is_some() || get_header(headers, "x-amz-cf-pop").is_some()
    {
        return CdnProvider::Cloudfront;
    }

    if get_header(headers, "x-vercel-id").is_some() {
        return CdnProvider::Vercel;
    }

    if get_header(headers, "cdn-pullzone").is_some() || server_contains(headers, "bunnycdn") {
        return CdnProvider::Bunny;
    }

    if get_header(headers, "x-nf-request-id").is_some() || server_contains(headers, "netlify") {
        return CdnProvider::Netlify;
    }

    if let Some(server_timing) = get_header(headers, "server-timing") {
        let lower = server_timing.to_ascii_lowercase();
        if lower.contains("cdn-cache") && server_contains(headers, "akamai") {
            return CdnProvider::Akamai;
        }
    }
    if has_header_with_prefix(headers, "x-akamai-") {
        return CdnProvider::Akamai;
    }

    if let Some(served_by) = get_header(headers, "x-served-by") {
        if served_by.to_ascii_lowercase().contains("cache-") {
            return CdnProvider::Fastly;
        }
    }
    if has_header_with_prefix(headers, "x-fastly-") {
        return CdnProvider::Fastly;
    }

    if server_contains(headers, "gcore") || get_header(headers, "x-id").is_some() {
        return CdnProvider::Gcore;
    }

    CdnProvider::None
}

pub fn parse_cache_status(headers: &HashMap<String, String>) -> Option<&'static str> {
    let raw = get_header(headers, "cf-cache-status")
        .or_else(|| get_header(headers, "x-vercel-cache"))
        .or_else(|| get_header(headers, "cdn-cache"))
        .or_else(|| get_header(headers, "x-cache"))
        .or_else(|| {
            get_header(headers, "server-timing").and_then(|s| extract_cdn_cache_desc(s))
        });

    let raw = raw?;
    Some(normalise_cache_status(raw))
}

fn normalise_cache_status(raw: &str) -> &'static str {
    let lower = raw.trim().to_ascii_lowercase();
    let first = lower.split([',', ' ', ';']).next().unwrap_or("").trim();

    if first.contains("hit") {
        "hit"
    } else if first.contains("miss") {
        "miss"
    } else if first.contains("stale") {
        "stale"
    } else if first.contains("expired") {
        "expired"
    } else if first.contains("bypass") || first.contains("pass") {
        "bypass"
    } else if first.contains("dynamic") || first.contains("uncacheable") {
        "dynamic"
    } else {
        "unknown"
    }
}

fn extract_cdn_cache_desc(server_timing: &str) -> Option<&str> {
    for entry in server_timing.split(',') {
        let entry = entry.trim();
        if !entry.to_ascii_lowercase().starts_with("cdn-cache") {
            continue;
        }
        for part in entry.split(';') {
            let part = part.trim();
            if let Some(desc) = part.strip_prefix("desc=").or_else(|| part.strip_prefix("desc =")) {
                return Some(desc.trim_matches('"'));
            }
        }
    }
    None
}

pub fn parse_edge_pop(headers: &HashMap<String, String>, provider: CdnProvider) -> Option<String> {
    match provider {
        CdnProvider::Cloudflare => {
            let ray = get_header(headers, "cf-ray")?;
            let suffix = ray.rsplit_once('-')?.1.trim();
            if suffix.is_empty() {
                None
            } else {
                Some(suffix.to_ascii_uppercase())
            }
        }
        CdnProvider::Cloudfront => {
            let pop = get_header(headers, "x-amz-cf-pop")?;
            let trimmed = pop.trim();
            if trimmed.len() < 3 {
                return None;
            }
            Some(trimmed[..3].to_ascii_uppercase())
        }
        CdnProvider::Fastly => {
            let served_by = get_header(headers, "x-served-by")?;
            let last = served_by.split(',').next_back()?.trim();
            for segment in last.split('-') {
                let trimmed = segment.trim();
                if trimmed.len() == 3 && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
                    return Some(trimmed.to_ascii_uppercase());
                }
            }
            None
        }
        CdnProvider::Vercel => {
            let id = get_header(headers, "x-vercel-id")?;
            let prefix = id.split([':', '-']).next()?.trim();
            if prefix.is_empty() {
                None
            } else {
                Some(prefix.to_ascii_uppercase())
            }
        }
        CdnProvider::Bunny => {
            let server = get_header(headers, "server")?;
            let suffix = server.rsplit_once('-')?.1.trim();
            if suffix.is_empty() {
                None
            } else {
                Some(suffix.to_ascii_uppercase())
            }
        }
        _ => None,
    }
}

pub fn parse_cache_age_seconds(headers: &HashMap<String, String>) -> Option<f64> {
    get_header(headers, "age")?.trim().parse::<f64>().ok()
}

fn get_header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn server_contains(headers: &HashMap<String, String>, needle: &str) -> bool {
    get_header(headers, "server")
        .map(|s| s.to_ascii_lowercase().contains(needle))
        .unwrap_or(false)
}

fn has_header_with_prefix(headers: &HashMap<String, String>, prefix: &str) -> bool {
    headers
        .keys()
        .any(|k| k.to_ascii_lowercase().starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn cloudflare_fixture() {
        let headers = h(&[
            ("cf-ray", "8a3f9b6d2e0fec3a-DXB"),
            ("cf-cache-status", "HIT"),
            ("server", "cloudflare"),
            ("age", "120"),
        ]);
        let provider = parse_cdn_provider(&headers);
        assert_eq!(provider, CdnProvider::Cloudflare);
        assert_eq!(parse_cache_status(&headers), Some("hit"));
        assert_eq!(parse_edge_pop(&headers, provider), Some("DXB".to_string()));
        assert_eq!(parse_cache_age_seconds(&headers), Some(120.0));
    }

    #[test]
    fn fastly_fixture() {
        let headers = h(&[
            ("x-served-by", "cache-fra-eddf8230084-FRA"),
            ("x-cache", "HIT, HIT"),
            ("x-cache-hits", "1, 0"),
            ("server", "fastly"),
        ]);
        let provider = parse_cdn_provider(&headers);
        assert_eq!(provider, CdnProvider::Fastly);
        assert_eq!(parse_cache_status(&headers), Some("hit"));
        assert_eq!(parse_edge_pop(&headers, provider), Some("FRA".to_string()));
    }

    #[test]
    fn cloudfront_fixture() {
        let headers = h(&[
            ("x-amz-cf-id", "abcdefg"),
            ("x-amz-cf-pop", "FRA50-C1"),
            ("x-cache", "Hit from cloudfront"),
        ]);
        let provider = parse_cdn_provider(&headers);
        assert_eq!(provider, CdnProvider::Cloudfront);
        assert_eq!(parse_cache_status(&headers), Some("hit"));
        assert_eq!(parse_edge_pop(&headers, provider), Some("FRA".to_string()));
    }

    #[test]
    fn akamai_fixture() {
        let headers = h(&[
            ("server-timing", "cdn-cache; desc=\"HIT\""),
            ("x-akamai-edgescape", "country_code=DE"),
            ("server", "AkamaiGHost"),
        ]);
        let provider = parse_cdn_provider(&headers);
        assert_eq!(provider, CdnProvider::Akamai);
        assert_eq!(parse_cache_status(&headers), Some("hit"));
    }

    #[test]
    fn vercel_fixture() {
        let headers = h(&[
            ("x-vercel-id", "fra1::abc123"),
            ("x-vercel-cache", "HIT"),
        ]);
        let provider = parse_cdn_provider(&headers);
        assert_eq!(provider, CdnProvider::Vercel);
        assert_eq!(parse_cache_status(&headers), Some("hit"));
        assert_eq!(parse_edge_pop(&headers, provider), Some("FRA1".to_string()));
    }

    #[test]
    fn bunny_fixture() {
        let headers = h(&[
            ("cdn-pullzone", "12345"),
            ("cdn-cache", "HIT"),
            ("server", "BunnyCDN-DE1"),
        ]);
        let provider = parse_cdn_provider(&headers);
        assert_eq!(provider, CdnProvider::Bunny);
        assert_eq!(parse_cache_status(&headers), Some("hit"));
        assert_eq!(parse_edge_pop(&headers, provider), Some("DE1".to_string()));
    }

    #[test]
    fn gcore_fixture() {
        let headers = h(&[("server", "gcore"), ("x-id", "edge-fra-01")]);
        assert_eq!(parse_cdn_provider(&headers), CdnProvider::Gcore);
    }

    #[test]
    fn netlify_fixture() {
        let headers = h(&[("x-nf-request-id", "01HZ..."), ("server", "Netlify")]);
        assert_eq!(parse_cdn_provider(&headers), CdnProvider::Netlify);
    }

    #[test]
    fn none_fixture() {
        let headers = h(&[("server", "nginx/1.25.0"), ("content-type", "text/html")]);
        assert_eq!(parse_cdn_provider(&headers), CdnProvider::None);
        assert_eq!(parse_cache_status(&headers), None);
        assert_eq!(parse_edge_pop(&headers, CdnProvider::None), None);
        assert_eq!(parse_cache_age_seconds(&headers), None);
    }

    #[test]
    fn case_insensitive_header_lookup() {
        let headers = h(&[("CF-Ray", "X-DXB"), ("Server", "Cloudflare")]);
        let provider = parse_cdn_provider(&headers);
        assert_eq!(provider, CdnProvider::Cloudflare);
        assert_eq!(parse_edge_pop(&headers, provider), Some("DXB".to_string()));
    }

    #[test]
    fn dynamic_cache_status_normalises() {
        let headers = h(&[("cf-cache-status", "DYNAMIC")]);
        assert_eq!(parse_cache_status(&headers), Some("dynamic"));
    }

    #[test]
    fn stale_cache_status_normalises() {
        let headers = h(&[("cf-cache-status", "STALE")]);
        assert_eq!(parse_cache_status(&headers), Some("stale"));
    }
}
