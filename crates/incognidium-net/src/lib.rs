use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use url::Url;

/// A single stored cookie.
#[derive(Debug, Clone)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    /// Domain the cookie is valid for. If `None`, derived from the request host.
    pub domain: Option<String>,
    /// Path prefix the cookie is valid for. If `None`, defaults to "/".
    pub path: Option<String>,
}

/// In-memory cookie jar used by HTTP requests and exposed to JavaScript via
/// `document.cookie`. Production code uses the global jar; tests can create
/// local jars for isolation.
#[derive(Debug, Default, Clone)]
pub struct CookieJar {
    cookies: Vec<StoredCookie>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self {
            cookies: Vec::new(),
        }
    }

    pub fn replace(&mut self, cookies: Vec<StoredCookie>) {
        self.cookies = cookies;
    }

    pub fn list(&self) -> Vec<StoredCookie> {
        self.cookies.clone()
    }

    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        if let Some(existing) = self.cookies.iter_mut().find(|c| c.name == name) {
            existing.value = value;
            return;
        }
        self.cookies.push(StoredCookie {
            name,
            value,
            domain: None,
            path: None,
        });
    }

    pub fn remove(&mut self, name: &str) {
        self.cookies.retain(|c| c.name != name);
    }

    /// Parse a `Set-Cookie` header value and store it.
    pub fn store_set_cookie(&mut self, raw: &str) {
        let raw = raw.trim();
        if raw.is_empty() {
            return;
        }
        let Some((name, rest)) = raw.split_once('=') else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let mut parts = rest.split(';');
        let value = parts.next().unwrap_or("").trim().to_string();
        let mut domain = None;
        let mut path = None;
        for part in parts {
            let part = part.trim();
            if let Some(v) = part.strip_prefix("domain=") {
                domain = Some(v.trim().to_string());
            } else if let Some(v) = part.strip_prefix("path=") {
                path = Some(v.trim().to_string());
            }
        }
        if let Some(existing) = self.cookies.iter_mut().find(|c| c.name == name) {
            existing.value = value;
            if domain.is_some() {
                existing.domain = domain;
            }
            if path.is_some() {
                existing.path = path;
            }
        } else {
            self.cookies.push(StoredCookie {
                name,
                value,
                domain,
                path,
            });
        }
    }

    /// Serialize the cookies that should be sent for `url` into a `Cookie`
    /// header value.
    pub fn cookie_header_for_url(&self, url: &Url) -> Option<String> {
        if self.cookies.is_empty() {
            return None;
        }
        let host = url.host_str().unwrap_or("").to_lowercase();
        let path = url.path();
        let mut parts = Vec::new();
        for c in self.cookies.iter() {
            let domain_match = c
                .domain
                .as_ref()
                .map(|d| {
                    host == d.to_lowercase() || host.ends_with(&format!(".{}", d.to_lowercase()))
                })
                .unwrap_or(true);
            let path_match = c.path.as_ref().map(|p| path.starts_with(p)).unwrap_or(true);
            if domain_match && path_match {
                parts.push(format!("{}={}", c.name, c.value));
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("; "))
        }
    }
}

static COOKIE_STORE: OnceLock<Mutex<CookieJar>> = OnceLock::new();

fn cookie_store() -> &'static Mutex<CookieJar> {
    COOKIE_STORE.get_or_init(|| Mutex::new(CookieJar::new()))
}

/// Replace the entire global in-memory cookie jar.
pub fn set_cookies(cookies: Vec<StoredCookie>) {
    cookie_store().lock().unwrap().replace(cookies);
}

/// Return a copy of every cookie in the global jar.
pub fn get_cookies() -> Vec<StoredCookie> {
    cookie_store().lock().unwrap().list()
}

/// Set or replace a cookie in the global jar by name.
pub fn set_cookie(name: impl Into<String>, value: impl Into<String>) {
    cookie_store().lock().unwrap().set(name, value);
}

/// Remove a cookie from the global jar by name.
pub fn delete_cookie(name: &str) {
    cookie_store().lock().unwrap().remove(name);
}

/// Serialize the cookies that should be sent for `url` from the global jar.
pub fn cookie_header_for_url(url: &Url) -> Option<String> {
    cookie_store().lock().unwrap().cookie_header_for_url(url)
}

/// Store a `Set-Cookie` value in the global jar.
pub fn store_set_cookie(raw: &str) {
    cookie_store().lock().unwrap().store_set_cookie(raw);
}

/// In-memory blob URL registry for `URL.createObjectURL` backed resources.
/// Maps `blob:incognidium/<uuid>` URLs to their content and MIME type.
#[derive(Debug, Clone)]
pub struct BlobEntry {
    pub data: Vec<u8>,
    pub content_type: String,
}

static BLOB_REGISTRY: OnceLock<Mutex<HashMap<String, BlobEntry>>> = OnceLock::new();

fn blob_registry() -> &'static Mutex<HashMap<String, BlobEntry>> {
    BLOB_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a synthetic blob URL that later fetches can resolve.
pub fn register_blob_url(url: &str, data: Vec<u8>, content_type: &str) {
    blob_registry().lock().unwrap().insert(
        url.to_string(),
        BlobEntry {
            data,
            content_type: content_type.to_string(),
        },
    );
}

fn fetch_blob_url(url_str: &str) -> Result<FetchResponse, String> {
    let entry = blob_registry()
        .lock()
        .unwrap()
        .get(url_str)
        .cloned()
        .ok_or_else(|| format!("Unknown blob URL: {}", url_str))?;
    Ok(FetchResponse {
        url: url_str.to_string(),
        status: 200,
        body: String::from_utf8_lossy(&entry.data).into_owned(),
        content_type: entry.content_type.clone(),
        headers: HashMap::new(),
    })
}

fn fetch_blob_url_bytes(url_str: &str) -> Result<Vec<u8>, String> {
    let entry = blob_registry()
        .lock()
        .unwrap()
        .get(url_str)
        .cloned()
        .ok_or_else(|| format!("Unknown blob URL: {}", url_str))?;
    Ok(entry.data)
}

/// Parse a `data:` URL and return its decoded body and declared media type.
///
/// Format: `data:[<mediatype>][;base64],<data>`.  If the payload is not
/// base64-encoded, percent-encoded bytes are decoded.
fn parse_data_url(url_str: &str) -> Result<(Vec<u8>, String), String> {
    let prefix = "data:";
    if !url_str.starts_with(prefix) {
        return Err(format!("Not a data URL: {}", url_str));
    }
    let rest = &url_str[prefix.len()..];
    let comma_idx = rest
        .find(',')
        .ok_or_else(|| "Invalid data URL: missing comma".to_string())?;
    let meta = &rest[..comma_idx];
    let payload = &rest[comma_idx + 1..];

    let base64 = meta
        .split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"));
    let media_type = meta
        .split(';')
        .find(|part| !part.is_empty() && !part.eq_ignore_ascii_case("base64"))
        .unwrap_or("text/plain;charset=US-ASCII")
        .to_string();

    if base64 {
        let data =
            base64_decode_url(payload).map_err(|e| format!("Invalid base64 data URL: {e}"))?;
        Ok((data, media_type))
    } else {
        let data =
            percent_decode_str(payload).map_err(|e| format!("Invalid percent encoding: {e}"))?;
        Ok((data, media_type))
    }
}

fn fetch_data_url(url_str: &str) -> Result<FetchResponse, String> {
    let (data, content_type) = parse_data_url(url_str)?;
    Ok(FetchResponse {
        url: url_str.to_string(),
        status: 200,
        body: String::from_utf8_lossy(&data).into_owned(),
        content_type,
        headers: HashMap::new(),
    })
}

fn fetch_data_url_bytes(url_str: &str) -> Result<Vec<u8>, String> {
    parse_data_url(url_str).map(|(data, _)| data)
}

fn base64_decode_str(input: &str) -> Result<Vec<u8>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    STANDARD.decode(input).map_err(|e| e.to_string())
}

fn base64_decode_url(input: &str) -> Result<Vec<u8>, String> {
    // Percent-encoded characters inside base64 data are unusual but tolerated.
    let normalized: String = input
        .bytes()
        .filter(|&b| !b.is_ascii_whitespace())
        .map(char::from)
        .collect();
    base64_decode_str(&normalized)
}

fn percent_decode_str(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let a = chars
                .next()
                .ok_or_else(|| "Truncated percent escape".to_string())?;
            let b = chars
                .next()
                .ok_or_else(|| "Truncated percent escape".to_string())?;
            let hi = a
                .to_digit(16)
                .ok_or_else(|| format!("Invalid percent escape: %{}{}", a, b))?;
            let lo = b
                .to_digit(16)
                .ok_or_else(|| format!("Invalid percent escape: %{}{}", a, b))?;
            out.push(((hi << 4) | lo) as u8);
        } else {
            // Spaces may be encoded as '+' in query-like contexts, but raw data URLs
            // use %20; keep literal '+' as-is for compatibility with some encoders.
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
        }
    }
    Ok(out)
}

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
        "blob" => fetch_blob_url(url_str),
        "data" => fetch_data_url(url_str),
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
        "blob" => fetch_blob_url(&req.url),
        "data" => fetch_data_url(&req.url),
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
        "blob" => fetch_blob_url_bytes(url_str),
        "data" => fetch_data_url_bytes(url_str),
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
        || relative.starts_with("data:")
        || relative.starts_with("blob:")
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
/// instance after a JS challenge is solved. Replacing it drops any stale
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
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(10))
        .cookie_store(true)
        .pool_max_idle_per_host(20)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    *guard = Some(client.clone());
    Ok(client)
}

/// Drop the current shared HTTP client so the next request creates a clean one.
/// Call this after installing a fresh challenge session to purge stale cookies.
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

    // If a challenge session has been solved, send its cookies and matching
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

        let has_accept = req.headers.keys().any(|k| k.eq_ignore_ascii_case("accept"));

        // Document-style browser headers on the first GET attempt.
        if attempt == 0 && req.method.eq_ignore_ascii_case("GET") && !has_accept {
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
        } else if req.method.eq_ignore_ascii_case("GET") && !has_accept {
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

        // Inject cookies from the engine-managed jar when the caller did not
        // provide an explicit Cookie header. This lets JS `fetch`/`XHR` keep
        // session state across requests without exposing the underlying HTTP
        // client directly.
        let has_cookie = req.headers.keys().any(|k| k.eq_ignore_ascii_case("cookie"));
        if !has_cookie {
            if let Some(cookie_str) = cookie_header_for_url(&url) {
                rb = rb.header("Cookie", cookie_str);
            }
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

                // Store any Set-Cookie headers in the shared jar so subsequent
                // JS/XHR/fetch calls keep the session alive.
                for cookie in resp.headers().get_all("set-cookie") {
                    if let Ok(raw) = cookie.to_str() {
                        store_set_cookie(raw);
                    }
                }

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

/// Fast single-attempt fetch for subresources such as stylesheets, scripts,
/// and XHRs. Subresources should not block the whole page for long, and they
/// do not need the heavy JS-challenge solving path used for main documents.
/// Any previously solved challenge session cookies are still sent so authenticated
/// subresources keep working.
pub fn fetch_url_fast(url_str: &str) -> Result<FetchResponse, String> {
    let url = parse_url(url_str)?;

    match url.scheme() {
        "blob" => return fetch_blob_url(url_str),
        "data" => return fetch_data_url(url_str),
        "file" => {
            let mut resp = fetch_file(&url)?;
            // fetch_file() defaults to text/html, but this fast path is used for
            // CSS fetches; update the content type from the path extension so the
            // caller gets a sensible hint.
            let path = url.to_file_path().map_err(|_| "Invalid file path")?;
            if path.extension().and_then(|e| e.to_str()) == Some("css") {
                resp.content_type = "text/css".into();
            }
            return Ok(resp);
        }
        _ => {}
    }

    let client = shared_http_client()?;
    let timeout = Duration::from_secs(10);

    let mut req = FetchRequest::get(url.as_str());
    apply_challenge_headers(&mut req);

    let method = match req.method.to_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        other => return Err(format!("Unsupported HTTP method: {other}")),
    };

    let mut rb = client
        .request(method, url.as_str())
        .timeout(timeout)
        .header("Accept", "text/css,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.5")
        .header("Cache-Control", "max-age=0");

    for (k, v) in &req.headers {
        if k.is_empty() {
            continue;
        }
        rb = rb.header(k, v);
    }

    let has_cookie = req.headers.keys().any(|k| k.eq_ignore_ascii_case("cookie"));
    if !has_cookie {
        if let Some(cookie_str) = cookie_header_for_url(&url) {
            rb = rb.header("Cookie", cookie_str);
        }
    }

    match rb.send() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let headers = collect_headers(&resp);
            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/css")
                .to_string();
            let final_url = resp.url().to_string();

            // Store any Set-Cookie headers in the shared jar so subresources
            // can maintain session state too.
            for cookie in resp.headers().get_all("set-cookie") {
                if let Ok(raw) = cookie.to_str() {
                    store_set_cookie(raw);
                }
            }

            let body = resp.text().map_err(|e| format!("Read error: {e}"))?;
            Ok(FetchResponse {
                url: final_url,
                body,
                content_type,
                status,
                headers,
            })
        }
        Err(e) => Err(format!("fast fetch failed for {url}: {e}")),
    }
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

/// Fetch an HTML partial fragment with the subresource headers some origins
/// require. This is a generic helper for `<*-partial src="...">` elements.
///
/// Partial endpoints often vend a dedicated MIME type and reply with HTTP 406
/// when the request does not advertise support for it. The Accept header lists
/// a generic partial-HTML type first and falls back to ordinary HTML. Callers
/// can override the Accept header for origin-specific variants.
pub fn fetch_html_partial(url_str: &str, referer: Option<&str>) -> Result<FetchResponse, String> {
    let url = parse_url(url_str)?;

    match url.scheme() {
        "blob" => fetch_blob_url(url_str),
        "data" => fetch_data_url(url_str),
        "file" => fetch_file(&url),
        "http" | "https" => {
            let mut req = FetchRequest::get(url.as_str());
            apply_challenge_headers(&mut req);
            req = req
                .header(
                    "Accept",
                    "text/vnd.incognidium.partial+html, text/html;q=0.9",
                )
                .header("Accept-Language", "en-US,en;q=0.5")
                .header("Cache-Control", "max-age=0")
                .header("Sec-Fetch-Dest", "empty")
                .header("Sec-Fetch-Mode", "cors")
                .header("Sec-Fetch-Site", "same-origin");
            if let Some(r) = referer {
                req = req.header("Referer", r);
            }
            fetch_http_with_request(&url, req)
        }
        scheme => Err(format!("Unsupported scheme: {scheme}")),
    }
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
        // Subresources should not block the whole page indefinitely. Keep the
        // timeout short and stop retrying once a request has timed out; a
        // server that did not answer in time is unlikely to answer on the next
        // attempt.
        let timeout = Duration::from_secs(15);

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
                if e.is_timeout() {
                    break;
                }
                continue;
            }
        }
    }

    Err(format!(
        "HTTP error after {FETCH_ATTEMPTS} attempts: {last_error}"
    ))
}

/// Cookies and headers extracted from a solved JS challenge. Installing a
/// session makes subsequent requests to the same site send the clearance
/// cookies so subresources and follow-up navigation also pass the challenge.
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

/// Clear any challenge session so normal requests are sent without challenge cookies.
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

/// Heuristic: does this response look like a generic JS-challenge
/// interstitial rather than real content?
fn looks_like_js_challenge(resp: &FetchResponse) -> bool {
    let body_lower = resp.body.to_ascii_lowercase();
    let known_marker = body_lower.contains("_cf_chl_opt")
        || body_lower.contains("cf-browser-verification")
        || body_lower.contains("<title>just a moment")
        || body_lower.contains("cf-challenge-running")
        || body_lower.contains("turnstile")
        || body_lower.contains("challenge.js")
        || body_lower.contains("managed_challenge");
    if known_marker {
        return true;
    }
    // Some challenge pages return 403/503 with a challenge marker header.
    let is_challenge_header = resp.headers.contains_key("cf-mitigated")
        || resp.headers.contains_key("cf-ray")
        || resp.headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("server") && v.to_ascii_lowercase().contains("cloudflare")
        });
    let is_challenge_status = resp.status == 403 || resp.status == 503;
    if is_challenge_header && is_challenge_status {
        return true;
    }

    // Generic anti-bot interstitial: a very small HTML page that contains a
    // form with a hidden "solution" field and a script that auto-submits it.
    // This pattern is used by several JS-challenge/challenge pages (not only
    // Cloudflare) and is not tied to any specific site text.
    let body_lower = resp.body.to_ascii_lowercase();
    let has_interstitial_form = body_lower.contains("<form")
        && body_lower.contains("<input")
        && body_lower.contains("name=\"solution\"")
        && body_lower.contains("</script>");
    if has_interstitial_form && resp.body.len() < 50_000 {
        return true;
    }

    false
}

/// Generic headless browser flags that reduce fingerprinting. These are the
/// standard anti-detection args used by headless Chromium tooling; they are
/// not site-specific.
fn headless_stealth_args() -> Vec<String> {
    vec![
        "--no-sandbox".to_string(),
        "--disable-setuid-sandbox".to_string(),
        "--disable-dev-shm-usage".to_string(),
        "--disable-gpu".to_string(),
        "--disable-blink-features=AutomationControlled".to_string(),
        "--disable-features=IsolateOrigins,site-per-process".to_string(),
        "--disable-site-isolation-trials".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--window-size=1920,1080".to_string(),
        "--start-maximized".to_string(),
        "--disable-background-networking".to_string(),
        "--disable-background-timer-throttling".to_string(),
        "--disable-renderer-backgrounding".to_string(),
        "--disable-client-side-phishing-detection".to_string(),
        "--disable-component-extensions-with-background-pages".to_string(),
    ]
}

/// Fetch a URL, detecting generic JS challenges and solving them with the
/// bundled stealth browser when one is present.
pub fn fetch_url_solving_challenges(url_str: &str) -> Result<FetchResponse, String> {
    let resp = fetch_url(url_str)?;
    if !looks_like_js_challenge(&resp) {
        return Ok(resp);
    }

    eprintln!("[net] JS challenge detected for {url_str}; solving with chaser-cf...");

    let (final_url, body) = {
        let rt = tokio::runtime::Runtime::new().map_err(|e| format!("tokio runtime: {e}"))?;
        rt.block_on(async {
            // Use a fresh browser profile directory for each solve so we never
            // collide with a stale SingletonLock left by a previous process.
            let profile_dir =
                std::env::temp_dir().join(format!("ingc_chrome_profile_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&profile_dir);
            let _ = std::fs::create_dir_all(&profile_dir);

            let mut extra = headless_stealth_args();
            extra.push(format!("--user-data-dir={}", profile_dir.to_string_lossy()));

            let mut config = chaser_cf::ChaserConfig::from_env()
                .with_headless(true)
                .with_timeout(std::time::Duration::from_secs(120));
            config.extra_args.extend(extra);

            let chaser = chaser_cf::ChaserCF::new(config)
                .await
                .map_err(|e| format!("chaser-cf init failed: {e:?}"))?;

            let result = solve_challenge_and_fetch(&chaser, url_str).await;
            // Always clean up the browser and the temporary profile directory.
            chaser.shutdown().await;
            let _ = std::fs::remove_dir_all(&profile_dir);
            result
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

async fn solve_challenge_and_fetch(
    chaser: &chaser_cf::ChaserCF,
    url_str: &str,
) -> Result<(String, String), String> {
    // Solve with retries: headless CDP timing can flake on the first
    // navigation, especially on heavily loaded challenge pages.
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
    let session = session_opt.ok_or_else(|| {
        format!("chaser-cf solve failed after {MAX_ATTEMPTS} attempts: {last_err}")
    })?;

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

    if !looks_like_js_challenge(&resp) {
        return Ok((resp.url, resp.body));
    }

    eprintln!(
        "[net] Challenge clearance not accepted by origin; fetching real source through chaser-cf browser."
    );
    let mut source = String::new();
    let mut get_source_err = String::new();
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
                get_source_err = msg;
                if attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
    }
    if source.is_empty() {
        return Err(format!(
            "chaser-cf get_source failed after {MAX_ATTEMPTS} attempts: {get_source_err}"
        ));
    }
    Ok((url_str.to_string(), source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

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

    #[test]
    fn test_parse_data_url_plain() {
        let (data, ct) = parse_data_url("data:text/plain,hello").unwrap();
        assert_eq!(data, b"hello");
        assert_eq!(ct, "text/plain");
    }

    #[test]
    fn test_parse_data_url_percent_encoded() {
        let (data, ct) = parse_data_url("data:text/javascript,%0Aconst%20x%20%3D%201%3B").unwrap();
        assert_eq!(data, b"\nconst x = 1;");
        assert_eq!(ct, "text/javascript");
    }

    #[test]
    fn test_parse_data_url_base64() {
        let (data, ct) = parse_data_url("data:text/html;base64,PGI+aGk8L2I+").unwrap();
        assert_eq!(data, b"<b>hi</b>");
        assert_eq!(ct, "text/html");
    }

    #[test]
    fn test_parse_data_url_invalid() {
        assert!(parse_data_url("data:text/plain").is_err());
        assert!(parse_data_url("not-data:").is_err());
    }

    #[test]
    fn test_fetch_url_data() {
        let resp = fetch_url("data:text/css,body%7Bcolor%3Ared%7D").unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.content_type, "text/css");
        assert_eq!(resp.body, "body{color:red}");
    }

    #[test]
    fn test_fetch_bytes_data() {
        let bytes = fetch_bytes("data:application/octet-stream;base64,SGVsbG8=").unwrap();
        assert_eq!(bytes, b"Hello");
    }

    #[test]
    fn test_resolve_url_data_unchanged() {
        let data = "data:text/javascript,console.log(1)";
        let resolved = resolve_url("https://example.com/", data).unwrap();
        assert_eq!(resolved, data);
    }

    #[test]
    fn test_cookie_jar_domain_and_path() {
        let mut jar = CookieJar::new();
        jar.replace(vec![
            StoredCookie {
                name: "session".to_string(),
                value: "abc".to_string(),
                domain: Some("example.com".to_string()),
                path: Some("/".to_string()),
            },
            StoredCookie {
                name: "other".to_string(),
                value: "xyz".to_string(),
                domain: Some("notexample.com".to_string()),
                path: None,
            },
        ]);

        let url = Url::parse("https://www.example.com/page").unwrap();
        let header = jar.cookie_header_for_url(&url).unwrap();
        assert!(header.contains("session=abc"));
        assert!(!header.contains("other=xyz"));
    }

    #[test]
    fn test_cookie_jar_default_cookie_matches_any_host() {
        let mut jar = CookieJar::new();
        jar.set("token", "123");
        let url = Url::parse("https://anything.example/path").unwrap();
        let header = jar.cookie_header_for_url(&url).unwrap();
        assert_eq!(header, "token=123");
    }

    #[test]
    fn test_cookie_jar_parses_set_cookie() {
        let mut jar = CookieJar::new();
        jar.store_set_cookie("session=abc; Domain=example.com; Path=/; Secure; HttpOnly");
        jar.store_set_cookie("prefs=dark; Path=/account");

        let url = Url::parse("https://www.example.com/page").unwrap();
        let header = jar.cookie_header_for_url(&url).unwrap();
        assert!(header.contains("session=abc"));
        assert!(!header.contains("prefs= dark"));

        let account_url = Url::parse("https://www.example.com/account/settings").unwrap();
        let account_header = jar.cookie_header_for_url(&account_url).unwrap();
        assert!(account_header.contains("session=abc"));
        assert!(account_header.contains("prefs=dark"));
    }

    #[test]
    fn test_fetch_html_partial_sends_partial_accept_header() {
        // Spin up a tiny local server that records the request headers and
        // replies with a partial-HTML body. This verifies the generic partial
        // fetch path advertises support for the `text/vnd.incognidium.partial+html`
        // MIME type instead of being clobbered by the default document Accept.
        let (tx, rx) = std::sync::mpsc::channel();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            let _ = tx.send(request.to_string());
            let body = "<p>partial fragment</p>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/vnd.incognidium.partial+html\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        });

        let url = format!("http://{}/partial", addr);
        let resp = fetch_html_partial(&url, Some("https://example.com/login")).unwrap();
        let request = rx.recv().unwrap();

        assert_eq!(resp.status, 200);
        assert!(resp.body.contains("partial fragment"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("accept: text/vnd.incognidium.partial+html, text/html;q=0.9"),
            "partial fetch should advertise text/vnd.incognidium.partial+html; got:\n{}",
            request
        );
    }

    mod challenge_tests {
        use super::*;

        #[test]
        fn detects_cloudflare_interstitial() {
            let resp = FetchResponse {
                url: "https://example.com".into(),
                body: r#"<html><head><script>window._cf_chl_opt={};</script></head><body><form action="/" method="POST"><input type="hidden" name="solution" value="foo"/></form><script>document.forms[0].submit();</script></body></html>"#.into(),
                content_type: "text/html".into(),
                status: 403,
                headers: HashMap::new(),
            };
            assert!(looks_like_js_challenge(&resp));
        }

        #[test]
        fn normal_page_is_not_challenge() {
            let resp = FetchResponse {
                url: "https://example.com".into(),
                body: r#"<html><head><title>Example Domain</title></head><body><h1>Example Domain</h1><p>This domain is for use in illustrative examples.</p></body></html>"#.into(),
                content_type: "text/html".into(),
                status: 200,
                headers: HashMap::new(),
            };
            assert!(!looks_like_js_challenge(&resp));
        }

        #[test]
        fn detects_403_with_cf_mitigated_header_and_form() {
            let mut headers = HashMap::new();
            headers.insert("cf-mitigated".to_string(), "challenge".to_string());
            let resp = FetchResponse {
                url: "https://example.com/protected".into(),
                body: r#"<html><body><form method="POST"><input name="solution" type="hidden" value="x"/></form></body></html>"#.into(),
                content_type: "text/html".into(),
                status: 403,
                headers,
            };
            assert!(looks_like_js_challenge(&resp));
        }
    }
}
