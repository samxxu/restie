// =============================================================================
// CLI Engine - command parsing, param distribution, path replacement
// =============================================================================

use crate::auth;
use crate::config::EndpointConfig;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug)]
pub struct RequestParts {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub body: Option<Value>,
    pub headers: HashMap<String, String>,
}

/// Replace {param} placeholders in a path template with actual values.
pub fn replace_path_placeholders(template: &str, params: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in params {
        let placeholder = format!("{{{}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
}

/// Strip control flags (--raw, --site, --config) out of a module's argv so
/// they never leak into the request payload as API parameters.
/// Returns the clean arg list plus the extracted values, if any.
pub fn strip_control_flags(args: &[String]) -> (Vec<String>, bool, Option<String>, Option<String>) {
    let mut filtered = Vec::with_capacity(args.len());
    let mut raw = false;
    let mut site = None;
    let mut config = None;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--raw" => {
                raw = true;
                i += 1;
            }
            "--site" | "--config" => {
                // --site <value> or --config <value>
                if i + 1 < args.len() {
                    let value = args[i + 1].clone();
                    if arg == "--site" {
                        site = Some(value);
                    } else {
                        config = Some(value);
                    }
                    i += 2;
                } else {
                    // Flag at end with no value: drop it.
                    i += 1;
                }
            }
            _ if arg.starts_with("--site=") => {
                site = Some(arg["--site=".len()..].to_string());
                i += 1;
            }
            _ if arg.starts_with("--config=") => {
                config = Some(arg["--config=".len()..].to_string());
                i += 1;
            }
            _ => {
                filtered.push(args[i].clone());
                i += 1;
            }
        }
    }

    (filtered, raw, site, config)
}

/// Distribute params into path / query / body based on config.
/// Fails (Err) when a required path parameter is missing, so callers can
/// report a clear error instead of sending a request with an unreplaced
/// `{placeholder}` still in the URL.
pub fn distribute_params_with_config(
    ep: &EndpointConfig,
    params: &HashMap<String, String>,
) -> Result<RequestParts, String> {
    let mut path_params = HashMap::new();
    let mut query_params = HashMap::new();
    let mut body_map = serde_json::Map::new();
    let mut header_params = HashMap::new();

    for (name, value) in params {
        let location = ep
            .params
            .get(name)
            .map(|p| p.location.as_str())
            .unwrap_or("query");

        match location {
            "path" => {
                path_params.insert(name.clone(), value.clone());
            }
            "query" => {
                query_params.insert(name.clone(), value.clone());
            }
            "body" => {
                let parsed: Value =
                    serde_json::from_str(value).unwrap_or(Value::String(value.clone()));
                body_map.insert(name.clone(), parsed);
            }
            "header" => {
                // Canonicalize to Title-Case so a header param never
                // duplicates an auth/static header under a different casing
                // (HTTP header names are case-insensitive).
                let canonical = auth::normalize_header_name(name);
                header_params.insert(canonical, value.clone());
            }
            _ => {
                query_params.insert(name.clone(), value.clone());
            }
        }
    }

    let path = replace_path_placeholders(&ep.path, &path_params);
    let body = if body_map.is_empty() {
        None
    } else {
        Some(Value::Object(body_map))
    };

    // Validate required path params are present and non-empty before sending.
    for (name, p) in &ep.params {
        if p.required && p.location == "path" {
            match path_params.get(name) {
                None => {
                    return Err(format!(
                        "Missing required parameter '--{}' for path '{}'",
                        name, ep.path
                    ));
                }
                Some(v) if v.trim().is_empty() => {
                    return Err(format!(
                        "Missing required parameter '--{}' (empty value) for path '{}'",
                        name, ep.path
                    ));
                }
                _ => {}
            }
        }
    }

    Ok(RequestParts {
        method: ep.method.to_uppercase(),
        path,
        query: query_params,
        body,
        headers: header_params,
    })
}

/// Distribute params using heuristic rules (no config).
pub fn distribute_params_heuristic(
    url_template: &str,
    method: &str,
    params: &HashMap<String, String>,
) -> RequestParts {
    let mut path_params = HashMap::new();
    let mut query_params = HashMap::new();
    let mut body: Option<Value> = None;

    for (name, value) in params {
        let placeholder = format!("{{{}}}", name);
        if url_template.contains(&placeholder) {
            path_params.insert(name.clone(), value.clone());
        } else if name == "body" {
            let parsed: Value = serde_json::from_str(value).unwrap_or(Value::String(value.clone()));
            body = Some(parsed);
        } else {
            query_params.insert(name.clone(), value.clone());
        }
    }

    let path = replace_path_placeholders(url_template, &path_params);

    RequestParts {
        method: method.to_uppercase(),
        path,
        query: query_params,
        body,
        headers: HashMap::new(),
    }
}

/// Execute a request using HttpClient.
pub async fn execute(
    client: &crate::http_client::HttpClient,
    parts: RequestParts,
) -> Result<Value, Box<dyn std::error::Error>> {
    let query = if parts.query.is_empty() {
        None
    } else {
        Some(parts.query)
    };

    let headers = if parts.headers.is_empty() {
        None
    } else {
        Some(&parts.headers)
    };

    match parts.method.as_str() {
        "GET" => {
            client
                .request_with_headers("GET", &parts.path, None, query, headers)
                .await
        }
        "POST" => {
            client
                .request_with_headers("POST", &parts.path, parts.body, query, headers)
                .await
        }
        "PUT" => {
            client
                .request_with_headers("PUT", &parts.path, parts.body, query, headers)
                .await
        }
        "PATCH" => {
            client
                .request_with_headers("PATCH", &parts.path, parts.body, query, headers)
                .await
        }
        "DELETE" => {
            client
                .request_with_headers("DELETE", &parts.path, None, query, headers)
                .await
        }
        _ => {
            client
                .request_with_headers("GET", &parts.path, None, query, headers)
                .await
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_path_placeholders() {
        let mut params = HashMap::new();
        params.insert("owner".to_string(), "rust-lang".to_string());
        params.insert("repo".to_string(), "rust".to_string());
        let path = replace_path_placeholders("/repos/{owner}/{repo}", &params);
        assert_eq!(path, "/repos/rust-lang/rust");
    }

    #[test]
    fn test_replace_path_placeholders_no_match() {
        let params = HashMap::new();
        let path = replace_path_placeholders("/repos/{owner}/{repo}", &params);
        assert_eq!(path, "/repos/{owner}/{repo}");
    }

    #[test]
    fn test_distribute_heuristic_path_params() {
        let mut params = HashMap::new();
        params.insert("owner".to_string(), "rust-lang".to_string());
        params.insert("repo".to_string(), "rust".to_string());
        params.insert("per_page".to_string(), "10".to_string());

        let parts = distribute_params_heuristic("/repos/{owner}/{repo}", "GET", &params);

        assert_eq!(parts.path, "/repos/rust-lang/rust");
        assert_eq!(parts.query.get("per_page"), Some(&"10".to_string()));
        assert!(parts.body.is_none());
    }

    #[test]
    fn test_distribute_heuristic_body_param() {
        let mut params = HashMap::new();
        params.insert("body".to_string(), r#"{"title":"bug"}"#.to_string());

        let parts = distribute_params_heuristic("/repos/x/y/issues", "POST", &params);

        assert!(parts.body.is_some());
        assert_eq!(parts.body.unwrap()["title"], "bug");
        assert!(parts.query.is_empty());
    }

    #[test]
    fn test_distribute_heuristic_all_query() {
        let mut params = HashMap::new();
        params.insert("q".to_string(), "rust".to_string());
        params.insert("per_page".to_string(), "10".to_string());

        let parts = distribute_params_heuristic("/search/repositories", "GET", &params);

        assert_eq!(parts.path, "/search/repositories");
        assert_eq!(parts.query.get("q"), Some(&"rust".to_string()));
        assert_eq!(parts.query.get("per_page"), Some(&"10".to_string()));
    }

    #[test]
    fn test_distribute_with_config() {
        use crate::config::*;
        let ep = EndpointConfig {
            method: "POST".to_string(),
            path: "/repos/{owner}/{repo}/issues".to_string(),
            summary: None,
            aliases: vec![],
            params: {
                let mut m = std::collections::BTreeMap::new();
                m.insert(
                    "owner".to_string(),
                    ParamConfig {
                        required: true,
                        location: "path".to_string(),
                        description: None,
                        param_type: None,
                        default: None,
                    },
                );
                m.insert(
                    "repo".to_string(),
                    ParamConfig {
                        required: true,
                        location: "path".to_string(),
                        description: None,
                        param_type: None,
                        default: None,
                    },
                );
                m.insert(
                    "title".to_string(),
                    ParamConfig {
                        required: true,
                        location: "body".to_string(),
                        description: None,
                        param_type: None,
                        default: None,
                    },
                );
                m
            },
        };

        let mut params = HashMap::new();
        params.insert("owner".to_string(), "rust-lang".to_string());
        params.insert("repo".to_string(), "rust".to_string());
        params.insert("title".to_string(), "bug report".to_string());

        let parts = distribute_params_with_config(&ep, &params).unwrap();

        assert_eq!(parts.method, "POST");
        assert_eq!(parts.path, "/repos/rust-lang/rust/issues");
        assert!(parts.query.is_empty());
        assert!(parts.body.is_some());
        assert_eq!(parts.body.unwrap()["title"], "bug report");
    }

    #[test]
    fn test_strip_control_flags_raw() {
        let args: Vec<String> = vec![
            "get".to_string(),
            "--owner".to_string(),
            "o".to_string(),
            "--raw".to_string(),
            "--per_page".to_string(),
            "10".to_string(),
        ];
        let (filtered, raw, site, config) = strip_control_flags(&args);
        assert!(raw);
        assert!(site.is_none());
        assert!(config.is_none());
        // --raw removed, everything else preserved in order.
        assert_eq!(
            filtered,
            vec![
                "get".to_string(),
                "--owner".to_string(),
                "o".to_string(),
                "--per_page".to_string(),
                "10".to_string()
            ]
        );
        // It is treated as a control flag, not a command.
        assert!(!filtered.contains(&"--raw".to_string()));
    }

    #[test]
    fn test_strip_control_flags_no_controls() {
        let args: Vec<String> = vec!["get".to_string(), "--owner".to_string(), "o".to_string()];
        let (filtered, raw, site, config) = strip_control_flags(&args);
        assert!(!raw);
        assert!(site.is_none());
        assert!(config.is_none());
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_strip_control_flags_site_and_config_forms() {
        // Space-separated form.
        let args: Vec<String> = vec![
            "get".to_string(),
            "--site".to_string(),
            "github".to_string(),
            "--owner".to_string(),
            "o".to_string(),
        ];
        let (filtered, _, site, _) = strip_control_flags(&args);
        assert_eq!(site.as_deref(), Some("github"));
        assert_eq!(
            filtered,
            vec!["get".to_string(), "--owner".to_string(), "o".to_string()]
        );

        // Equals form.
        let args: Vec<String> = vec![
            "get".to_string(),
            "--site=github".to_string(),
            "--config=/tmp/x.yaml".to_string(),
        ];
        let (filtered, _, site, config) = strip_control_flags(&args);
        assert_eq!(site.as_deref(), Some("github"));
        assert_eq!(config.as_deref(), Some("/tmp/x.yaml"));
        assert_eq!(filtered, vec!["get".to_string()]);

        // Flag at the end with no value: dropped, not leaked as a param.
        let args: Vec<String> = vec!["get".to_string(), "--site".to_string()];
        let (filtered, _, site, _) = strip_control_flags(&args);
        assert!(site.is_none());
        assert_eq!(filtered, vec!["get".to_string()]);
    }

    #[test]
    fn test_distribute_with_config_header_params() {
        use crate::config::*;
        let ep = EndpointConfig {
            method: "GET".to_string(),
            path: "/items".to_string(),
            summary: None,
            aliases: vec![],
            params: {
                let mut m = std::collections::BTreeMap::new();
                m.insert(
                    "x-request-id".to_string(),
                    ParamConfig {
                        required: true,
                        location: "header".to_string(),
                        description: None,
                        param_type: None,
                        default: None,
                    },
                );
                m.insert(
                    "page".to_string(),
                    ParamConfig {
                        required: false,
                        location: "query".to_string(),
                        description: None,
                        param_type: None,
                        default: None,
                    },
                );
                m
            },
        };

        let mut params = HashMap::new();
        params.insert("x-request-id".to_string(), "abc-123".to_string());
        params.insert("page".to_string(), "2".to_string());

        let parts = distribute_params_with_config(&ep, &params).unwrap();
        // Header names are canonicalized to Title-Case.
        assert_eq!(
            parts.headers.get("X-Request-Id"),
            Some(&"abc-123".to_string())
        );
        assert_eq!(parts.query.get("page"), Some(&"2".to_string()));
        // Header params must NOT leak into query.
        assert!(!parts.query.contains_key("x-request-id"));
    }

    #[test]
    fn test_distribute_with_config_missing_required_path_param() {
        use crate::config::*;
        let ep = EndpointConfig {
            method: "GET".to_string(),
            path: "/repos/{owner}/{repo}".to_string(),
            summary: None,
            aliases: vec![],
            params: {
                let mut m = std::collections::BTreeMap::new();
                m.insert(
                    "owner".to_string(),
                    ParamConfig {
                        required: true,
                        location: "path".to_string(),
                        description: None,
                        param_type: None,
                        default: None,
                    },
                );
                m.insert(
                    "repo".to_string(),
                    ParamConfig {
                        required: true,
                        location: "path".to_string(),
                        description: None,
                        param_type: None,
                        default: None,
                    },
                );
                m
            },
        };

        // Only repo provided — owner path param is missing.
        let mut params = HashMap::new();
        params.insert("repo".to_string(), "rust".to_string());

        let err = distribute_params_with_config(&ep, &params).unwrap_err();
        assert!(
            err.contains("--owner"),
            "error should name owner, got: {}",
            err
        );
    }

    #[test]
    fn test_distribute_with_config_empty_required_path_param() {
        use crate::config::*;
        let ep = EndpointConfig {
            method: "GET".to_string(),
            path: "/repos/{owner}/{repo}".to_string(),
            summary: None,
            aliases: vec![],
            params: {
                let mut m = std::collections::BTreeMap::new();
                m.insert(
                    "owner".to_string(),
                    ParamConfig {
                        required: true,
                        location: "path".to_string(),
                        description: None,
                        param_type: None,
                        default: None,
                    },
                );
                m.insert(
                    "repo".to_string(),
                    ParamConfig {
                        required: true,
                        location: "path".to_string(),
                        description: None,
                        param_type: None,
                        default: None,
                    },
                );
                m
            },
        };

        // Both provided but owner is an empty string — must be rejected so the
        // URL never contains a dangling `/repos//rust` segment.
        let mut params = HashMap::new();
        params.insert("owner".to_string(), "".to_string());
        params.insert("repo".to_string(), "rust".to_string());

        let err = distribute_params_with_config(&ep, &params).unwrap_err();
        assert!(
            err.contains("--owner"),
            "error should name owner, got: {}",
            err
        );
        assert!(
            err.contains("empty"),
            "error should mention empty value, got: {}",
            err
        );
    }
}
