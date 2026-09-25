// =============================================================================
// HTTP Client - core call() + GET/POST/PUT/PATCH/DELETE convenience wrappers
//
// Authentication is driven by AuthConfig from the site config.
// Supports: bearer_token, api_key, custom_header, script, none
// =============================================================================

use crate::auth;
use crate::config::{AuthConfig, SiteConfig};
use serde_json::Value;
use std::collections::HashMap;

/// User-Agent sent on every request, including spec downloads.
pub const USER_AGENT: &str = concat!("restie/", env!("CARGO_PKG_VERSION"));

pub struct HttpClient {
    base_url: String,
    auth: Option<AuthConfig>,
    static_headers: Option<std::collections::BTreeMap<String, String>>,
    http: reqwest::Client,
}

#[allow(non_snake_case)]
impl HttpClient {
    pub fn new(config: &SiteConfig) -> Self {
        let base_url = config.get_base_url();
        let auth = config.get_auth().cloned();
        let static_headers = config.get_headers().cloned();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            base_url,
            auth,
            static_headers,
            http,
        }
    }

    pub fn with_base(base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth: None,
            static_headers: None,
            http,
        }
    }

    /// Core method - all HTTP wrappers delegate here.
    async fn call(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<Value>,
        query: Option<HashMap<String, String>>,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        // Build the full URL with reqwest's query encoding so the auth script
        // sees exactly the same query string as the actual request.
        let mut url = reqwest::Url::parse(&format!("{}{}", self.base_url, endpoint))
            .map_err(|e| format!("Invalid URL: {}", e))?;
        if let Some(ref params) = query {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in params {
                pairs.append_pair(k, v);
            }
        }

        // Query string as sent on the wire (for auth script signature context).
        // `query()` returns the raw query without the leading '?'.
        let query_str = url.query().map(|s| s.to_string());

        let body_str = body.as_ref().map(|b| b.to_string());

        // Build auth headers from config
        let auth_headers = auth::build_auth_headers(
            self.auth.as_ref(),
            method,
            endpoint,
            query_str.as_deref(),
            body_str.as_deref(),
        )?;

        let mut req = self
            .http
            .request(
                method
                    .parse::<reqwest::Method>()
                    .unwrap_or(reqwest::Method::GET),
                url.clone(),
            )
            .header("User-Agent", USER_AGENT);

        // Inject auth headers
        for (name, value) in &auth_headers {
            req = req.header(name, value);
        }

        // Inject static headers from config
        if let Some(ref headers) = self.static_headers {
            for (name, value) in headers {
                req = req.header(name, value);
            }
        }

        // Inject per-request headers (e.g. from config-defined header params)
        if let Some(extra) = extra_headers {
            for (name, value) in extra {
                req = req.header(name, value);
            }
        }

        if let Some(body_val) = body {
            req = req.json(&body_val);
        }

        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            let error_msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v["message"]
                        .as_str()
                        .map(|s| s.to_string())
                        .or_else(|| serde_json::to_string_pretty(&v).ok())
                })
                .unwrap_or_else(|| {
                    // Non-JSON error — keep it readable by trimming whitespace
                    // and capping at 500 chars so huge HTML error pages
                    // don't flood the terminal.
                    let trimmed = text.trim();
                    if trimmed.len() > 500 {
                        format!(
                            "{}... (truncated, {} bytes total)",
                            &trimmed[..500],
                            trimmed.len()
                        )
                    } else {
                        trimmed.to_string()
                    }
                });
            return Err(format!("API Error ({}): {}", status, error_msg).into());
        }

        // 204 No Content and empty responses → null, not an empty string.
        let result: Value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        Ok(result)
    }

    // -- HTTP method wrappers (uppercase, matching HTTP conventions) --

    pub async fn GET(
        &self,
        endpoint: &str,
        query: Option<HashMap<String, String>>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        self.call("GET", endpoint, None, query, None).await
    }

    pub async fn POST(
        &self,
        endpoint: &str,
        body: Option<Value>,
        query: Option<HashMap<String, String>>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        self.call("POST", endpoint, body, query, None).await
    }

    pub async fn PUT(
        &self,
        endpoint: &str,
        body: Option<Value>,
        query: Option<HashMap<String, String>>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        self.call("PUT", endpoint, body, query, None).await
    }

    pub async fn PATCH(
        &self,
        endpoint: &str,
        body: Option<Value>,
        query: Option<HashMap<String, String>>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        self.call("PATCH", endpoint, body, query, None).await
    }

    pub async fn DELETE(
        &self,
        endpoint: &str,
        query: Option<HashMap<String, String>>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        self.call("DELETE", endpoint, None, query, None).await
    }

    /// Execute a request with per-request headers (e.g. for header params).
    pub async fn request_with_headers(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<Value>,
        query: Option<HashMap<String, String>>,
        headers: Option<&HashMap<String, String>>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        self.call(method, endpoint, body, query, headers).await
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_base_trims_trailing_slash() {
        let client = HttpClient::with_base("https://api.example.com/");
        assert_eq!(client.base_url, "https://api.example.com");
    }

    #[test]
    fn test_with_base_no_trailing_slash() {
        let client = HttpClient::with_base("https://api.example.com");
        assert_eq!(client.base_url, "https://api.example.com");
    }
}
