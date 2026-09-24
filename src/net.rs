//! HTTPS for plugin code (ADR-0008). The policy here is pure: which URLs a plugin may
//! reach, how often, and how much; `Fetch` only moves the bytes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Largest response body a plugin gets; more fuel than it has could not parse more.
pub const BODY_CAP: usize = 2 << 20;
pub const REQUEST_BODY_CAP: usize = 64 << 10;
const REFUSED_HEADERS: &[&str] = &["host", "cookie", "connection", "content-length", "transfer-encoding", "te", "upgrade", "proxy-authorization", "proxy-connection"];

/// A host a plugin may reach: `api.example.com`, or `*.example.com` for its subdomains.
#[derive(Clone, Debug, PartialEq)]
pub enum HostPattern {
    Exact(String),
    Subdomains(String),
}

fn valid_host(h: &str) -> Result<(), String> {
    let labels: Vec<&str> = h.split('.').collect();
    let ok = labels.len() >= 2 && labels.iter().all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
    if !ok {
        return Err(format!("`{h}` is not a host name"));
    }
    if labels.iter().all(|l| l.bytes().all(|b| b.is_ascii_digit())) || labels.last().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit())) {
        return Err(format!("`{h}`: use a host name, not an IP address"));
    }
    if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") || h.ends_with(".internal") {
        return Err(format!("`{h}` is a local name"));
    }
    Ok(())
}

impl HostPattern {
    pub fn parse(s: &str) -> Result<HostPattern, String> {
        let s = s.trim().to_ascii_lowercase();
        match s.strip_prefix("*.") {
            Some(rest) => {
                valid_host(rest)?;
                Ok(HostPattern::Subdomains(rest.to_string()))
            }
            None if s.contains('*') => Err(format!("`{s}`: a wildcard may only be `*.` at the start")),
            None if s.contains(':') => Err(format!("`{s}`: no ports; plugins use HTTPS on 443")),
            None => {
                valid_host(&s)?;
                Ok(HostPattern::Exact(s))
            }
        }
    }

    pub fn matches(&self, host: &str) -> bool {
        match self {
            HostPattern::Exact(h) => host == h,
            HostPattern::Subdomains(h) => host.strip_suffix(h.as_str()).is_some_and(|sub| sub.len() > 1 && sub.ends_with('.')),
        }
    }
}

impl std::fmt::Display for HostPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostPattern::Exact(h) => write!(f, "{h}"),
            HostPattern::Subdomains(h) => write!(f, "*.{h}"),
        }
    }
}

/// Where an allowed request goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub host: String,
    /// Path and query, from the first `/`.
    pub path: String,
}

/// An `https://` URL to a listed host, strictly: no user info, ports, escapes in the host or
/// backslashes, which is where URL parsers disagree.
pub fn check_url(url: &str, allowed: &[HostPattern]) -> Result<Target, String> {
    let rest = url.get(..8).filter(|s| s.eq_ignore_ascii_case("https://")).map(|_| &url[8..]).ok_or_else(|| format!("`{url}`: only https:// URLs"))?;
    if url.bytes().any(|b| b <= b' ' || b == b'\\' || b >= 0x7f) {
        return Err(format!("`{url}` has spaces, backslashes or non-ASCII characters"));
    }
    let rest = rest.split('#').next().unwrap_or("");
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    if authority.contains(['@', '%', ':']) {
        return Err(format!("`{url}`: no user names, escapes or ports in the host"));
    }
    let host = authority.to_ascii_lowercase();
    valid_host(&host)?;
    if !allowed.iter().any(|p| p.matches(&host)) {
        return Err(format!("`{host}` is not in the plugin's net list"));
    }
    let path = match path {
        "" => "/".to_string(),
        p if p.starts_with('?') => format!("/{p}"),
        p => p.to_string(),
    };
    Ok(Target { host, path })
}

/// A burst of requests, then one per interval.
#[derive(Debug)]
pub struct RateLimit {
    tokens: f64,
    burst: f64,
    every: Duration,
    last: Instant,
}

impl RateLimit {
    pub fn new(burst: u32, every: Duration, now: Instant) -> RateLimit {
        RateLimit { tokens: burst as f64, burst: burst as f64, every, last: now }
    }

    pub fn take(&mut self, now: Instant) -> bool {
        let refill = now.saturating_duration_since(self.last).as_secs_f64() / self.every.as_secs_f64();
        self.tokens = (self.tokens + refill).min(self.burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub body: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    /// A redirect is handed back, never followed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub body: String,
}

/// Moves one checked request over the wire.
pub trait Fetch: Send + Sync {
    fn fetch(&self, to: &Target, req: &Request) -> Result<Response, String>;
}

/// One plugin's network: its host list, its rate limit, and the transport.
pub struct Net {
    allowed: Vec<HostPattern>,
    fetch: Arc<dyn Fetch>,
    rate: Mutex<RateLimit>,
}

fn error_json(e: &str) -> Vec<u8> {
    serde_json::json!({ "error": e }).to_string().into_bytes()
}

impl Net {
    pub fn new(allowed: Vec<HostPattern>, fetch: Arc<dyn Fetch>) -> Net {
        Net { allowed, fetch, rate: Mutex::new(RateLimit::new(20, Duration::from_secs(6), Instant::now())) }
    }

    /// A request from the module (JSON) to its response (JSON); every refusal is an `error`.
    pub fn handle(&self, request: &[u8]) -> Vec<u8> {
        match self.checked(request) {
            Ok(r) => serde_json::to_vec(&r).unwrap_or_else(|e| error_json(&e.to_string())),
            Err(e) => error_json(&e),
        }
    }

    fn checked(&self, request: &[u8]) -> Result<Response, String> {
        let mut req: Request = serde_json::from_slice(request).map_err(|e| format!("bad request: {e}"))?;
        req.method = if req.method.is_empty() { "GET".into() } else { req.method.to_ascii_uppercase() };
        if !matches!(req.method.as_str(), "GET" | "POST") {
            return Err(format!("{} is not allowed; use GET or POST", req.method));
        }
        if req.body.as_ref().is_some_and(|b| b.len() > REQUEST_BODY_CAP) {
            return Err("the request body is over 64 KB".into());
        }
        for (k, v) in &req.headers {
            let name_ok = !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            if !name_ok || v.contains(['\r', '\n']) || REFUSED_HEADERS.contains(&k.to_ascii_lowercase().as_str()) || k.to_ascii_lowercase().starts_with("proxy-") {
                return Err(format!("header `{k}` is not allowed"));
            }
        }
        let to = check_url(&req.url, &self.allowed)?;
        if !self.rate.lock().unwrap().take(Instant::now()) {
            return Err("too many requests; try again in a few seconds".into());
        }
        let mut resp = self.fetch.fetch(&to, &req)?;
        if resp.body.len() > BODY_CAP {
            return Err(format!("the response is over {} MB", BODY_CAP >> 20));
        }
        resp.location = resp.location.filter(|_| (300..400).contains(&resp.status));
        Ok(resp)
    }
}

/// A transport that answers from a closure, for tests.
#[cfg(test)]
pub(crate) struct FakeFetch(pub Box<dyn Fn(&Target, &Request) -> Result<Response, String> + Send + Sync>);

#[cfg(test)]
impl Fetch for FakeFetch {
    fn fetch(&self, to: &Target, req: &Request) -> Result<Response, String> {
        (self.0)(to, req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pats(p: &[&str]) -> Vec<HostPattern> {
        p.iter().map(|s| HostPattern::parse(s).unwrap()).collect()
    }

    #[test]
    fn host_patterns_match_exact_and_subdomains_only() {
        let exact = HostPattern::parse("API.Open-Meteo.com").unwrap();
        assert!(exact.matches("api.open-meteo.com"));
        assert!(!exact.matches("x.api.open-meteo.com") && !exact.matches("open-meteo.com"));
        let sub = HostPattern::parse("*.example.com").unwrap();
        assert!(sub.matches("a.example.com") && sub.matches("a.b.example.com"));
        assert!(!sub.matches("example.com"), "the bare domain needs its own entry");
        assert!(!sub.matches("badexample.com") && !sub.matches("example.com.evil.net"));
        assert_eq!(sub.to_string(), "*.example.com");
    }

    #[test]
    fn patterns_refuse_ips_localhost_ports_and_bare_wildcards() {
        for bad in ["*", "*.com", "10.0.0.1", "127.0.0.1", "localhost", "printer.local", "example.com:8443", "a*.example.com", "exa mple.com", "ex_ample.com", "münchen.de", "", "com"] {
            assert!(HostPattern::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_https_to_listed_hosts_passes() {
        let allowed = pats(&["api.open-meteo.com", "*.feeds.example.org"]);
        let t = check_url("https://api.open-meteo.com/v1/forecast?latitude=59.9#x", &allowed).unwrap();
        assert_eq!((t.host.as_str(), t.path.as_str()), ("api.open-meteo.com", "/v1/forecast?latitude=59.9"));
        assert_eq!(check_url("HTTPS://API.open-meteo.com", &allowed).unwrap().path, "/");
        assert_eq!(check_url("https://api.open-meteo.com?x=1", &allowed).unwrap().path, "/?x=1");
        assert!(check_url("https://news.feeds.example.org/rss", &allowed).is_ok());
        assert!(check_url("http://api.open-meteo.com/", &allowed).unwrap_err().contains("https"));
        assert!(check_url("https://evil.com/", &allowed).unwrap_err().contains("net list"));
        assert!(check_url("file:///C:/Windows/win.ini", &allowed).is_err());
    }

    #[test]
    fn userinfo_and_backslash_tricks_are_refused() {
        let allowed = pats(&["api.open-meteo.com"]);
        for bad in [
            "https://api.open-meteo.com@evil.com/",
            "https://evil.com@api.open-meteo.com/",
            "https://api.open-meteo.com\\@evil.com/",
            "https://api.open-meteo.com:444/",
            "https://api%2eopen-meteo.com/",
            "https://api.open-meteo.com /x",
            "https://127.0.0.1/",
        ] {
            assert!(check_url(bad, &allowed).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_rate_limit_refills() {
        let t0 = Instant::now();
        let mut r = RateLimit::new(2, Duration::from_secs(6), t0);
        assert!(r.take(t0) && r.take(t0));
        assert!(!r.take(t0), "burst spent");
        assert!(!r.take(t0 + Duration::from_secs(5)));
        assert!(r.take(t0 + Duration::from_secs(12)), "one back per 6 s");
    }

    #[test]
    fn a_redirect_is_returned_not_followed() {
        let fake = FakeFetch(Box::new(|to, req| {
            assert_eq!((to.host.as_str(), req.method.as_str()), ("api.open-meteo.com", "GET"));
            Ok(Response { status: 302, content_type: String::new(), location: Some("https://elsewhere.com/".into()), body: String::new() })
        }));
        let net = Net::new(pats(&["api.open-meteo.com"]), Arc::new(fake));
        let out: serde_json::Value = serde_json::from_slice(&net.handle(br#"{"url":"https://api.open-meteo.com/x"}"#)).unwrap();
        assert_eq!((out["status"].as_u64(), out["location"].as_str()), (Some(302), Some("https://elsewhere.com/")));
    }

    #[test]
    fn requests_are_checked_before_they_leave() {
        let fake = FakeFetch(Box::new(|_, _| Ok(Response { status: 200, content_type: "application/json".into(), location: None, body: "{}".into() })));
        let net = Net::new(pats(&["api.open-meteo.com"]), Arc::new(fake));
        let err = |req: &str| serde_json::from_slice::<serde_json::Value>(&net.handle(req.as_bytes())).unwrap()["error"].as_str().map(String::from);
        assert_eq!(err(r#"{"url":"https://api.open-meteo.com/","headers":{"Accept":"application/json"}}"#), None);
        assert!(err(r#"{"method":"DELETE","url":"https://api.open-meteo.com/"}"#).unwrap().contains("GET or POST"));
        assert!(err(r#"{"url":"https://api.open-meteo.com/","headers":{"Cookie":"a=b"}}"#).unwrap().contains("Cookie"));
        assert!(err(r#"{"url":"https://api.open-meteo.com/","headers":{"X-A":"b\r\nHost: evil"}}"#).is_some());
        assert!(err("not json").unwrap().contains("bad request"));
        let big = FakeFetch(Box::new(|_, _| Ok(Response { status: 200, body: "x".repeat(BODY_CAP + 1), ..Default::default() })));
        let net = Net::new(pats(&["api.open-meteo.com"]), Arc::new(big));
        let out: serde_json::Value = serde_json::from_slice(&net.handle(br#"{"url":"https://api.open-meteo.com/"}"#)).unwrap();
        assert!(out["error"].as_str().unwrap().contains("MB"));
    }
}
