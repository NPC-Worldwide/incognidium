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
        "http" | "https" => {
            let client = reqwest::blocking::Client::builder()
                .user_agent(USER_AGENT)
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .map_err(|e| format!("HTTP client error: {e}"))?;
            let resp = client
                .get(url.as_str())
                .send()
                .map_err(|e| format!("HTTP error: {e}"))?;
            resp.bytes()
                .map(|b| b.to_vec())
                .map_err(|e| format!("Read error: {e}"))
        }
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
}

fn fetch_file(url: &Url) -> Result<FetchResponse, String> {
    let path = url.to_file_path().map_err(|_| "Invalid file path")?;
    let body = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    Ok(FetchResponse {
        url: url.to_string(),
        body,
        content_type: "text/html".into(),
    })
}

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; incognidium/0.1) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 incognidium-qa (https://github.com/cjpais/incognidium; contact: cjp.agostino@gmail.com)";

fn fetch_http(url: &Url) -> Result<FetchResponse, String> {
    let _original_host = url.host_str().unwrap_or("").to_string();

    // Follow redirects (up to 10 hops)
    let policy = reqwest::redirect::Policy::limited(10);

    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .redirect(policy)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get(url.as_str())
        .send()
        .map_err(|e| format!("HTTP error: {e}"))?;

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/html")
        .to_string();

    let final_url = resp.url().to_string();
    let body = resp.text().map_err(|e| format!("Read error: {e}"))?;

    Ok(FetchResponse {
        url: final_url,
        body,
        content_type,
    })
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
