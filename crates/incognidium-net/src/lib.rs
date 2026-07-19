use url::Url;

/// Fetch a URL and return the response body as a string.
pub fn fetch_url(url_str: &str) -> Result<FetchResponse, String> {
    let url = parse_url(url_str)?;

    match url.scheme() {
        "file" => fetch_file(&url),
        "http" | "https" => fetch_http(&url),
        scheme => Err(format!("Unsupported scheme: {scheme}")),
    }
}

/// Fetch a resource as raw bytes (for images, etc).
pub fn fetch_bytes(url_str: &str) -> Result<Vec<u8>, String> {
    let url = parse_url(url_str)?;

    match url.scheme() {
        "file" => {
            let path = url.to_file_path().map_err(|_| "Invalid file path")?;
            std::fs::read(&path).map_err(|e| format!("Failed to read {}: {e}", path.display()))
        }
        "http" | "https" => fetch_bytes_http(&url),
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
    })
}

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// Number of fetch attempts before giving up.
const FETCH_ATTEMPTS: usize = 3;

/// Build an HTTP client for a given attempt. Later attempts are more permissive
/// (HTTP/1.1 only, longer timeouts) to tolerate sites with flaky HTTP/2 or
/// slow TLS handshakes.
fn build_http_client(attempt: usize) -> Result<reqwest::blocking::Client, String> {
    let timeout_secs = match attempt {
        0 => 15,
        1 => 30,
        _ => 45,
    };
    let connect_secs = match attempt {
        0 => 10,
        1 => 20,
        _ => 30,
    };

    let mut builder = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(connect_secs))
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(10))
        .cookie_store(true);

    if attempt >= 1 {
        // Some sites / middleboxes have broken or flaky HTTP/2. Force HTTP/1.1
        // on retries to avoid ALPN-related stalls and resets.
        builder = builder.http1_only();
    }

    builder
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))
}

fn fetch_http(url: &Url) -> Result<FetchResponse, String> {
    let mut last_error = String::new();

    for attempt in 0..FETCH_ATTEMPTS {
        let client = build_http_client(attempt)?;

        let mut req = client.get(url.as_str());
        if attempt == 0 {
            // Realistic browser headers on the first try.
            req = req
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
        } else {
            // Strip some fingerprinting headers on retries; a few CDNs block
            // requests that carry every Sec-Fetch hint but no cookie session.
            req = req
                .header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                )
                .header("Accept-Language", "en-US,en;q=0.5")
                .header("Cache-Control", "max-age=0");
        }

        match req.send() {
            Ok(resp) => {
                let content_type = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("text/html")
                    .to_string();
                let status = resp.status().as_u16();
                let final_url = resp.url().to_string();
                let body = resp.text().map_err(|e| format!("Read error: {e}"))?;
                return Ok(FetchResponse {
                    url: final_url,
                    body,
                    content_type,
                    status,
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

fn fetch_bytes_http(url: &Url) -> Result<Vec<u8>, String> {
    let mut last_error = String::new();

    for attempt in 0..FETCH_ATTEMPTS {
        let timeout = std::time::Duration::from_secs(if attempt == 0 { 10 } else { 20 });
        let connect = std::time::Duration::from_secs(if attempt == 0 { 5 } else { 15 });

        let mut builder = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(connect)
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(10))
            .cookie_store(true);
        if attempt >= 1 {
            builder = builder.http1_only();
        }

        let client = builder
            .build()
            .map_err(|e| format!("HTTP client error: {e}"))?;

        match client
            .get(url.as_str())
            .header("Accept", "image/webp,image/apng,image/*,*/*;q=0.8")
            .header("Referer", "https://www.google.com/")
            .send()
        {
            Ok(resp) => {
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
