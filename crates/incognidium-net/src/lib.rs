use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use url::Url;

/// A configurable HTTP fetch request.
#[derive(Debug, Default)]
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    pub body: Option<Vec<u8>>,
    pub headers: HashMap<String, String>,
}

impl FetchRequest {
    /// Build a GET request for a URL.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: "GET".into(),
            body: None,
            headers: HashMap::new(),
        }
    }

    /// Build a POST request with an optional body.
    pub fn post(url: impl Into<String>, body: Option<Vec<u8>>) -> Self {
        Self {
            url: url.into(),
            method: "POST".into(),
            body,
            headers: HashMap::new(),
        }
    }

    /// Add or replace a request header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }
}

/// Fetch a URL and return the response body as a string.
pub fn fetch_url(url_str: &str) -> Result<FetchResponse, String> {
    let url = parse_url(url_str)?;

    match url.scheme() {
        "file" => fetch_file(&url),
        "http" | "https" => fetch_http(&url),
        scheme => Err(format!("Unsupported scheme: {scheme}")),
    }
}

/// Perform an HTTP request with full control over method, body and headers.
///
/// This is the low-level entry point used by the shell's JavaScript `fetch` and
/// `XMLHttpRequest` bindings, and by the high-level `fetch_url` wrapper.  All
/// HTTP traffic shares one in-memory cookie jar so clearance cookies set by a
/// challenge response are sent on the redirected follow-up request.
pub fn fetch_with_options(req: FetchRequest) -> Result<FetchResponse, String> {
    let url = parse_url(&req.url)?;

    match url.scheme() {
        "file" => fetch_file(&url),
        "http" | "https" => fetch_http_with_request(&url, req),
        scheme => Err(format!("Unsupported scheme: {scheme}")),
    }
}

/// Fetch a resource as raw bytes (for images, etc).
pub fn fetch_bytes(url_str: &str) -> Result<Vec<u8>, String> {
    fetch_bytes_with_referer(url_str, None)
}

/// Fetch a resource as raw bytes with an optional Referer header.
///
/// Subresource requests should send the document URL as the referer so servers
/// that check hotlinking or rate-limit by page session can allow the request.
pub fn fetch_bytes_with_referer(url_str: &str, referer: Option<&str>) -> Result<Vec<u8>, String> {
    let url = parse_url(url_str)?;

    match url.scheme() {
        "file" => {
            let path = url.to_file_path().map_err(|_| "Invalid file path")?;
            std::fs::read(&path).map_err(|e| format!("Failed to read {}: {e}", path.display()))
        }
        "http" | "https" => fetch_bytes_http(&url, referer),
        scheme => Err(format!("Unsupported scheme: {scheme}")),
    }
}

/// Resolve a potentially relative URL against a base URL.
pub fn resolve_url(base: &str, relative: &str) -> Result<String, String> {
    // Already absolute
    if relative.starts_with("http://")
        || relative.starts_with("https://")
        || relative.starts_with("file://")
    {
        return Ok(relative.to_string());
    }
    let base_url = Url::parse(base).map_err(|e| format!("Invalid base URL: {e}"))?;
    let resolved = base_url
        .join(relative)
        .map_err(|e| format!("Failed to resolve URL: {e}"))?;
    Ok(resolved.to_string())
}

pub fn parse_url(input: &str) -> Result<Url, String> {
    // Try as-is first
    if let Ok(url) = Url::parse(input) {
        return Ok(url);
    }
    // Try as file path
    if input.starts_with('/') || input.starts_with('.') {
        let abs = if input.starts_with('/') {
            input.to_string()
        } else {
            let cwd = std::env::current_dir().map_err(|e| format!("{e}"))?;
            cwd.join(input).to_string_lossy().to_string()
        };
        return Url::from_file_path(&abs).map_err(|_| format!("Invalid file path: {abs}"));
    }
    // Assume https://
    let with_scheme = format!("https://{input}");
    Url::parse(&with_scheme).map_err(|e| format!("Invalid URL '{input}': {e}"))
}

#[derive(Debug)]
pub struct FetchResponse {
    pub url: String,
    pub body: String,
    pub content_type: String,
    pub status: u16,
    pub headers: HashMap<String, String>,
}

fn fetch_file(url: &Url) -> Result<FetchResponse, String> {
    let path = url.to_file_path().map_err(|_| "Invalid file path")?;
    let body = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    Ok(FetchResponse {
        url: url.to_string(),
        body,
        content_type: "text/html".into(),
        status: 200,
        headers: HashMap::new(),
    })
}

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// Number of fetch attempts before giving up.
const FETCH_ATTEMPTS: usize = 3;

static HTTP_CLIENT: OnceLock<Mutex<Option<reqwest::blocking::Client>>> = OnceLock::new();

fn http_client_store() -> &'static Mutex<Option<reqwest::blocking::Client>> {
    HTTP_CLIENT.get_or_init(|| Mutex::new(None))
}

/// Shared HTTP client for every request (main documents, subresources, XHR).
///
/// The client is stored behind a `Mutex` so it can be replaced with a fresh
/// instance after a WAF challenge is solved. Replacing it drops any stale
/// challenge cookies that accumulated before the clearance cookie was
/// obtained, which is required to avoid sending conflicting tokens to the
/// protected origin.
fn shared_http_client() -> Result<reqwest::blocking::Client, String> {
    let mut guard = http_client_store().lock().unwrap();
    if let Some(client) = guard.as_ref() {
        return Ok(client.clone());
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .cookie_store(true)
        .pool_max_idle_per_host(20)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    *guard = Some(client.clone());
    Ok(client)
}

/// Drop the current shared HTTP client so the next request creates a clean one.
/// Call this after installing a fresh WAF session to purge stale cookies.
pub fn reset_http_client() {
    *http_client_store().lock().unwrap() = None;
}

fn fetch_http(url: &Url) -> Result<FetchResponse, String> {
    fetch_http_with_request(
        url,
        FetchRequest {
            url: url.to_string(),
            method: "GET".into(),
            body: None,
            headers: HashMap::new(),
        },
    )
}

fn fetch_http_with_request(url: &Url, mut req: FetchRequest) -> Result<FetchResponse, String> {
    let mut last_error = String::new();

    // If a WAF session has been solved, send its cookies and matching
    // user-agent automatically.
    apply_challenge_headers(&mut req);

    for attempt in 0..FETCH_ATTEMPTS {
        let client = shared_http_client()?;

        let timeout_secs = match attempt {
            0 => 15,
            1 => 30,
            _ => 45,
        };

        let method = match req.method.to_uppercase().as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            "HEAD" => reqwest::Method::HEAD,
            "OPTIONS" => reqwest::Method::OPTIONS,
            other => return Err(format!("Unsupported HTTP method: {other}")),
        };

        let mut rb = client
            .request(method, url.as_str())
            .timeout(Duration::from_secs(timeout_secs));

        // Document-style browser headers on the first GET attempt.
        if attempt == 0 && req.method.eq_ignore_ascii_case("GET") {
            rb = rb
                .header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                )
                .header("Accept-Language", "en-US,en;q=0.5")
                .header("DNT", "1")
                .header("Connection", "keep-alive")
                .header("Upgrade-Insecure-Requests", "1")
                .header("Sec-Fetch-Dest", "document")
                .header("Sec-Fetch-Mode", "navigate")
                .header("Sec-Fetch-Site", "none")
                .header("Sec-Fetch-User", "?1")
                .header("Cache-Control", "max-age=0");
        } else if req.method.eq_ignore_ascii_case("GET") {
            // Strip some fingerprinting headers on retries; a few CDNs block
            // requests that carry every Sec-Fetch hint but no cookie session.
            rb = rb
                .header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                )
                .header("Accept-Language", "en-US,en;q=0.5")
                .header("Cache-Control", "max-age=0");
        }

        for (k, v) in &req.headers {
            // reqwest panics when a header name is empty; skip malformed entries
            // injected by misbehaving scripts.
            if k.is_empty() {
                continue;
            }
            rb = rb.header(k, v);
        }

        if let Some(body) = &req.body {
            rb = rb.body(body.clone());
        }

        match rb.send() {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let headers = collect_headers(&resp);
                let content_type = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("text/html")
                    .to_string();
                let final_url = resp.url().to_string();
                let body = resp.text().map_err(|e| format!("Read error: {e}"))?;
                return Ok(FetchResponse {
                    url: final_url,
                    body,
                    content_type,
                    status,
                    headers,
                });
            }
            Err(e) => {
                last_error = format!("attempt {attempt}: {e}");
                eprintln!("[net] {url}: {last_error}");
                continue;
            }
        }
    }

    Err(format!(
        "HTTP error after {FETCH_ATTEMPTS} attempts: {last_error}"
    ))
}

fn collect_headers(resp: &reqwest::blocking::Response) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for (k, v) in resp.headers().iter() {
        if let Ok(s) = v.to_str() {
            headers.insert(k.as_str().to_lowercase(), s.to_string());
        }
    }
    headers
}

/// Minimum gap between requests to the same host. Spacing subresource fetches
/// avoids tripping CDN per-IP rate-limiters (HTTP 429) when a page references
/// many images or icons from one origin.
const MIN_HOST_REQUEST_INTERVAL: Duration = Duration::from_millis(150);

/// Global map tracking the next allowed request time per host.
fn host_request_schedule() -> &'static Mutex<HashMap<String, Instant>> {
    static SCHEDULE: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    SCHEDULE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Wait until at least `MIN_HOST_REQUEST_INTERVAL` has elapsed since the last
/// request to this URL's host. The lock is dropped before sleeping so other
/// threads can enqueue concurrently.
fn throttle_per_host(url: &Url) {
    let host = url.host_str().map(|h| h.to_string()).unwrap_or_default();
    if host.is_empty() {
        return;
    }

    let sleep_for = {
        let now = Instant::now();
        let mut schedule = host_request_schedule().lock().unwrap();
        let next_allowed = schedule.get(&host).copied().unwrap_or(now);
        let wait = if next_allowed > now {
            next_allowed.duration_since(now)
        } else {
            Duration::ZERO
        };
        schedule.insert(host, now + wait + MIN_HOST_REQUEST_INTERVAL);
        wait
    };

    if sleep_for > Duration::ZERO {
        std::thread::sleep(sleep_for);
    }
}

fn fetch_bytes_http(url: &Url, referer: Option<&str>) -> Result<Vec<u8>, String> {
    let mut last_error = String::new();

    for attempt in 0..FETCH_ATTEMPTS {
        // Space subresource fetches per host to avoid CDN rate-limiting.
        throttle_per_host(url);

        let client = shared_http_client()?;
        let timeout = Duration::from_secs(30);

        let (cookie_str, challenge_ua) = {
            let session = challenge_session().lock().unwrap();
            let cookie_str = if session.cookies.is_empty() {
                None
            } else {
                Some(
                    session
                        .cookies
                        .iter()
                        .map(|(name, value)| format!("{}={}", name, value))
                        .collect::<Vec<_>>()
                        .join("; "),
                )
            };
            let ua = session.headers.get("user-agent").cloned();
            (cookie_str, ua)
        };

        let mut req = client
            .get(url.as_str())
            .timeout(timeout)
            .header("Accept", "image/webp,image/apng,image/*,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.5");
        if let Some(r) = referer {
            req = req.header("Referer", r);
        }
        if let Some(cookie_str) = cookie_str {
            req = req.header("Cookie", cookie_str);
        }
        if let Some(ua) = challenge_ua {
            req = req.header("User-Agent", ua);
        }

        match req.send() {
            Ok(resp) => {
                let status = resp.status();
                if !status.is_success() {
                    last_error = format!("attempt {attempt}: HTTP {status}");
                    eprintln!("[net bytes] {url}: {last_error}");
                    // Rate-limit responses benefit from a longer, adaptive backoff
                    // rather than a fixed 100 ms retry.
                    let is_ratelimit = status.as_u16() == 429 || status.as_u16() == 503;
                    let delay = if is_ratelimit {
                        let retry_after = resp
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .map(|secs| Duration::from_secs(secs.saturating_add(1)));
                        retry_after.unwrap_or_else(|| {
                            Duration::from_millis(250u64.saturating_mul(1 << attempt).min(5000))
                        })
                    } else {
                        Duration::from_millis(100)
                    };
                    std::thread::sleep(delay);
                    continue;
                }
                let bytes = resp.bytes().map_err(|e| format!("Read error: {e}"))?;
                return Ok(bytes.to_vec());
            }
            Err(e) => {
                last_error = format!("attempt {attempt}: {e}");
                eprintln!("[net bytes] {url}: {last_error}");
                continue;
            }
        }
    }

    Err(format!(
        "HTTP error after {FETCH_ATTEMPTS} attempts: {last_error}"
    ))
}

/// Cookies and headers extracted from a solved WAF challenge. Installing a
/// session makes subsequent requests to the same site send the clearance
/// cookies so subresources and follow-up navigation also pass the WAF.
#[derive(Debug, Clone, Default)]
pub struct ChallengeSession {
    pub cookies: Vec<(String, String)>,
    pub headers: HashMap<String, String>,
}

static CHALLENGE_SESSION: OnceLock<Mutex<ChallengeSession>> = OnceLock::new();

fn challenge_session() -> &'static Mutex<ChallengeSession> {
    CHALLENGE_SESSION.get_or_init(|| Mutex::new(ChallengeSession::default()))
}

/// Replace the currently installed challenge session.
pub fn install_challenge_session(session: ChallengeSession) {
    *challenge_session().lock().unwrap() = session;
}

/// Clear any challenge session so normal requests are sent without WAF cookies.
pub fn clear_challenge_session() {
    *challenge_session().lock().unwrap() = ChallengeSession::default();
}

/// Inject the installed challenge `Cookie` and `User-Agent` headers when the
/// request does not already carry them. The clearance cookie is bound to the
/// browser fingerprint that solved the challenge, so requests must use the
/// same user-agent for the cookie to be accepted.
fn apply_challenge_headers(req: &mut FetchRequest) {
    let session = challenge_session().lock().unwrap();

    let has_cookie = req.headers.keys().any(|k| k.eq_ignore_ascii_case("cookie"));
    if !has_cookie && !session.cookies.is_empty() {
        let cookie_str = session
            .cookies
            .iter()
            .map(|(name, value)| format!("{}={}", name, value))
            .collect::<Vec<_>>()
            .join("; ");
        req.headers.insert("Cookie".to_string(), cookie_str);
    }

    let has_ua = req
        .headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("user-agent"));
    if !has_ua {
        if let Some(ua) = session.headers.get("user-agent") {
            req.headers.insert("User-Agent".to_string(), ua.clone());
        }
    }
}

/// Heuristic: does this response look like a Cloudflare JS challenge
/// interstitial rather than real content?
fn looks_like_cloudflare_challenge(resp: &FetchResponse) -> bool {
    let body_lower = resp.body.to_ascii_lowercase();
    let known_marker = body_lower.contains("_cf_chl_opt")
        || body_lower.contains("cf-browser-verification")
        || body_lower.contains("<title>just a moment")
        || body_lower.contains("cf-challenge-running");
    if known_marker {
        return true;
    }
    // Some challenge pages return 403 with the managed-challenge header.
    let is_cf_mitigated =
        resp.headers.contains_key("cf-mitigated") || resp.headers.contains_key("cf-ray");
    let is_challenge_status = resp.status == 403 || resp.status == 503;
    is_cf_mitigated && is_challenge_status
}

/// Fetch a URL, detecting Cloudflare-managed JS challenges and solving them
/// when the `challenge-solver` feature is enabled.
#[cfg(feature = "challenge-solver")]
pub fn fetch_url_solving_challenges(url_str: &str) -> Result<FetchResponse, String> {
    let resp = fetch_url(url_str)?;
    if !looks_like_cloudflare_challenge(&resp) {
        return Ok(resp);
    }

    eprintln!("[net] Cloudflare challenge detected for {url_str}; solving with chaser-cf...");

    let (final_url, body) = {
        let rt = tokio::runtime::Runtime::new().map_err(|e| format!("tokio runtime: {e}"))?;
        rt.block_on(async {
            let mut config = chaser_cf::ChaserConfig::from_env()
                .with_headless(true)
                .with_timeout(std::time::Duration::from_secs(120));
            // Servers without a display need headless Chrome; WAF pages sometimes
            // detect headless and stall, so add common anti-detection flags.
            config.extra_args.extend([
                "--no-sandbox".to_string(),
                "--disable-setuid-sandbox".to_string(),
                "--disable-dev-shm-usage".to_string(),
                "--disable-gpu".to_string(),
                "--disable-blink-features=AutomationControlled".to_string(),
            ]);

            let chaser = chaser_cf::ChaserCF::new(config)
                .await
                .map_err(|e| format!("chaser-cf init failed: {e:?}"))?;

            // Solve with retries: headless CDP timing can flake on the first
            // navigation, especially on heavily loaded WAF pages.
            const MAX_ATTEMPTS: usize = 3;
            let mut last_err = String::new();
            let mut session_opt: Option<chaser_cf::models::WafSession> = None;
            for attempt in 1..=MAX_ATTEMPTS {
                eprintln!("[net] chaser-cf solve attempt {attempt}/{MAX_ATTEMPTS}...");
                match chaser.solve_waf_session(url_str, None).await {
                    Ok(session) => {
                        session_opt = Some(session);
                        break;
                    }
                    Err(e) => {
                        let msg = format!("{e:?}");
                        eprintln!("[net] chaser-cf solve attempt {attempt} failed: {msg}");
                        last_err = msg;
                        if attempt < MAX_ATTEMPTS {
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        }
                    }
                }
            }
            let session = session_opt.ok_or_else(|| format!("chaser-cf solve failed after {MAX_ATTEMPTS} attempts: {last_err}"))?;

            let cookies: Vec<(String, String)> = session
                .cookies
                .iter()
                .map(|c| (c.name.clone(), c.value.clone()))
                .collect();
            install_challenge_session(ChallengeSession {
                cookies,
                headers: session.headers.clone(),
            });

            // A fresh HTTP client ensures no stale challenge cookies from the
            // initial detection request are sent alongside the new clearance cookie.
            reset_http_client();

            eprintln!("[net] Challenge solved; re-fetching {url_str} with clearance cookies.");
            let mut req = FetchRequest::get(url_str);
            apply_challenge_headers(&mut req);
            let resp = fetch_with_options(req)?;

            if !looks_like_cloudflare_challenge(&resp) {
                let final_url = resp.url;
                let body = resp.body;
                chaser.shutdown().await;
                return Ok::<_, String>((final_url, body));
            }

            eprintln!(
                "[net] Clearance cookie not accepted by origin; fetching real source through chaser-cf browser."
            );
            let mut source = String::new();
            for attempt in 1..=MAX_ATTEMPTS {
                eprintln!("[net] chaser-cf get_source attempt {attempt}/{MAX_ATTEMPTS}...");
                match chaser.get_source(url_str, None).await {
                    Ok(s) => {
                        source = s;
                        break;
                    }
                    Err(e) => {
                        let msg = format!("{e:?}");
                        eprintln!("[net] chaser-cf get_source attempt {attempt} failed: {msg}");
                        last_err = msg;
                        if attempt < MAX_ATTEMPTS {
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        }
                    }
                }
            }
            if source.is_empty() {
                return Err(format!(
                    "chaser-cf get_source failed after {MAX_ATTEMPTS} attempts: {last_err}"
                ));
            }
            chaser.shutdown().await;
            Ok((url_str.to_string(), source))
        })?
    };

    Ok(FetchResponse {
        url: final_url,
        body,
        content_type: "text/html".into(),
        status: 200,
        headers: HashMap::new(),
    })
}

/// Without the solver feature, `fetch_url_solving_challenges` behaves exactly
/// like `fetch_url` so pages still render the challenge interstitial when the
/// engine cannot complete it.
#[cfg(not(feature = "challenge-solver"))]
pub fn fetch_url_solving_challenges(url_str: &str) -> Result<FetchResponse, String> {
    fetch_url(url_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url_https() {
        let url = parse_url("example.com").unwrap();
        assert_eq!(url.scheme(), "https");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_parse_url_file() {
        let url = parse_url("/tmp/test.html").unwrap();
        assert_eq!(url.scheme(), "file");
    }

    #[test]
    fn test_resolve_url() {
        let resolved = resolve_url("https://example.com/page/", "image.png").unwrap();
        assert_eq!(resolved, "https://example.com/page/image.png");
    }
}
