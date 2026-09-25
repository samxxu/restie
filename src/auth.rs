// =============================================================================
// Auth - authentication header injection
//
// Supports:
//   bearer_token  → Authorization: Bearer <token from env>
//   api_key       → <header>: <prefix> <token from env>
//   custom_header → <header>: <token from env>  (no prefix)
//   script        → call external script, parse stdout for headers
//   none          → no auth headers
// =============================================================================

use crate::config::AuthConfig;
use std::collections::HashMap;
use std::env;

/// Result of authentication: a set of headers to inject into the request.
pub type AuthHeaders = HashMap<String, String>;

/// Generate auth headers based on the AuthConfig.
/// For script type, this calls the external script with RESTIE_* env vars.
/// For other types, this reads the token from the env var and constructs headers.
pub fn build_auth_headers(
    auth: Option<&AuthConfig>,
    method: &str,
    path: &str,
    query: Option<&str>,
    body: Option<&str>,
) -> Result<AuthHeaders, Box<dyn std::error::Error>> {
    let auth = match auth {
        Some(a) => a,
        None => return Ok(AuthHeaders::new()),
    };

    match auth.auth_type.as_str() {
        "none" => Ok(AuthHeaders::new()),

        "bearer_token" => {
            let token = read_token(auth)?;
            let header = auth.header.as_deref().unwrap_or("Authorization");
            let prefix = auth.prefix.as_deref().unwrap_or("Bearer");
            let mut headers = AuthHeaders::new();
            if prefix.is_empty() {
                headers.insert(header.to_string(), token);
            } else {
                headers.insert(header.to_string(), format!("{} {}", prefix, token));
            }
            Ok(headers)
        }

        "api_key" => {
            let token = read_token(auth)?;
            let header = auth.header.as_deref().unwrap_or("Authorization");
            let prefix = auth.prefix.as_deref().unwrap_or("");
            let mut headers = AuthHeaders::new();
            if prefix.is_empty() {
                headers.insert(header.to_string(), token);
            } else {
                headers.insert(header.to_string(), format!("{} {}", prefix, token));
            }
            Ok(headers)
        }

        "custom_header" => {
            let token = read_token(auth)?;
            let header = auth.header.as_deref().unwrap_or("Authorization");
            let mut headers = AuthHeaders::new();
            headers.insert(header.to_string(), token);
            Ok(headers)
        }

        "script" => {
            let script_path = auth
                .command
                .as_deref()
                .ok_or("auth.command not set for script type")?;
            call_auth_script(script_path, auth, method, path, query, body)
        }

        _ => {
            eprintln!(
                "Warning: unknown auth type '{}', skipping auth",
                auth.auth_type
            );
            Ok(AuthHeaders::new())
        }
    }
}

/// Read token from the environment variable specified in token_env.
fn read_token(auth: &AuthConfig) -> Result<String, Box<dyn std::error::Error>> {
    let env_name = auth.token_env.as_deref().ok_or("auth.token_env not set")?;
    env::var(env_name).map_err(|_| format!("Environment variable {} not set", env_name).into())
}

/// Call an external auth script.
///
/// The script receives these env vars:
///   RESTIE_METHOD, RESTIE_PATH, RESTIE_QUERY, RESTIE_BODY
///   Plus any vars from auth.env mapping
///
/// The script should output headers, one per line: "Header: value"
fn call_auth_script(
    script_path: &str,
    auth: &AuthConfig,
    method: &str,
    path: &str,
    query: Option<&str>,
    body: Option<&str>,
) -> Result<AuthHeaders, Box<dyn std::error::Error>> {
    // Expand ~ in script path.
    let expanded = expand_tilde(script_path);
    let script = expanded.as_deref().unwrap_or(script_path).to_string();

    // Bare relative paths resolve against the config directory first (where
    // bundled/installed and user-edited scripts live), then against the presets
    // cache (~/.config/restie/presets/, where `presets update` stages repo
    // scripts) so a downloaded signing script runs without being copied to
    // auth/. Absolute paths and inline `sh -c` commands fall through untouched.
    let script = if !std::path::Path::new(&script).is_absolute()
        && !std::path::Path::new(&script).is_file()
    {
        let config_hit = crate::config::SiteConfig::config_dir().join(&script);
        if config_hit.is_file() {
            config_hit.to_string_lossy().to_string()
        } else {
            let cache_hit = crate::config::SiteConfig::config_dir()
                .join("presets")
                .join(&script);
            if cache_hit.is_file() {
                cache_hit.to_string_lossy().to_string()
            } else {
                script
            }
        }
    } else {
        script
    };

    // If the command points to an existing file, exec it directly (no shell),
    // which prevents command injection and handles paths with spaces correctly.
    // Otherwise fall back to `sh -c` for inline scripts — the user is responsible
    // for the safety of their own config.
    let mut cmd = if std::path::Path::new(&script).is_file() {
        std::process::Command::new(&script)
    } else {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(&script);
        c
    };

    // Set RESTIE_* context vars
    cmd.env("RESTIE_METHOD", method);
    cmd.env("RESTIE_PATH", path);
    cmd.env("RESTIE_QUERY", query.unwrap_or(""));
    cmd.env("RESTIE_BODY", body.unwrap_or(""));

    // Set additional env vars from config
    if let Some(ref env_map) = auth.env {
        for (key, value) in env_map {
            // Expand ${VAR} references
            let expanded = expand_env(value);
            cmd.env(key, expanded);
        }
    }

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run auth script: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Auth script failed: {}", stderr).into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut headers = AuthHeaders::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = normalize_header_name(name.trim());
            headers.insert(name, value.trim().to_string());
        }
    }

    Ok(headers)
}

/// Normalize a header name to Title-Case so the same header never appears
/// twice with different casing (e.g. `authorization` vs `Authorization`).
/// HTTP header names are case-insensitive, so we pick one canonical form.
pub fn normalize_header_name(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let first = first.to_uppercase().collect::<String>();
                    format!("{}{}", first, chars.as_str().to_lowercase())
                }
                None => String::new(),
            }
        })
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Expand a leading `~` in a path to the user's home directory.
/// Returns None if the path does not start with `~` or HOME is not set.
fn expand_tilde(path: &str) -> Option<String> {
    let home = env::var("HOME").ok()?;
    path.strip_prefix('~')
        .map(|rest| format!("{}{}", home, rest))
}

/// Expand ${VAR} references in a string using environment variables.
fn expand_env(s: &str) -> String {
    let mut result = s.to_string();
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let value = std::env::var(var_name).unwrap_or_else(|_| {
                eprintln!(
                    "Warning: environment variable '{}' is not set (used in auth config)",
                    var_name
                );
                String::new()
            });
            result = format!(
                "{}{}{}",
                &result[..start],
                value,
                &result[start + end + 1..]
            );
        } else {
            break;
        }
    }
    result
}

// =============================================================================
// Signing scripts
//
// RESTie ships NO signing scripts. `script` auth resolves its command against
// the user's ~/.config/restie/auth/ first, then the presets cache
// (~/.config/restie/presets/), and both are populated from the presets
// repository (https://github.com/samxxu/restie-presets) by
// `restie presets update`. Keeping them out of the binary is what lets a
// platform's signing logic change without a RESTie release.
// =============================================================================

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_auth(auth_type: &str) -> AuthConfig {
        AuthConfig {
            auth_type: auth_type.to_string(),
            token_env: Some("TEST_TOKEN".to_string()),
            header: None,
            prefix: None,
            command: None,
            env: None,
        }
    }

    #[test]
    fn test_none_auth() {
        let auth = make_auth("none");
        let headers = build_auth_headers(Some(&auth), "GET", "/test", None, None).unwrap();
        assert!(headers.is_empty());
    }

    #[test]
    fn test_bearer_token_auth() {
        std::env::set_var("TEST_TOKEN_A", "my-token-123");
        let mut auth = make_auth("bearer_token");
        auth.token_env = Some("TEST_TOKEN_A".to_string());
        let headers = build_auth_headers(Some(&auth), "GET", "/test", None, None).unwrap();
        assert_eq!(
            headers.get("Authorization"),
            Some(&"Bearer my-token-123".to_string())
        );
    }

    #[test]
    fn test_api_key_custom_header() {
        std::env::set_var("TEST_TOKEN_B", "key-abc");
        let mut auth = make_auth("api_key");
        auth.token_env = Some("TEST_TOKEN_B".to_string());
        auth.header = Some("X-API-Key".to_string());
        auth.prefix = Some("".to_string());
        let headers = build_auth_headers(Some(&auth), "GET", "/test", None, None).unwrap();
        // Empty prefix → no leading space, raw token in the header.
        assert_eq!(headers.get("X-API-Key"), Some(&"key-abc".to_string()));
    }

    #[test]
    fn test_custom_header_no_prefix() {
        std::env::set_var("TEST_TOKEN_C", "gitlab-token");
        let mut auth = make_auth("custom_header");
        auth.token_env = Some("TEST_TOKEN_C".to_string());
        auth.header = Some("PRIVATE-TOKEN".to_string());
        let headers = build_auth_headers(Some(&auth), "GET", "/test", None, None).unwrap();
        assert_eq!(
            headers.get("PRIVATE-TOKEN"),
            Some(&"gitlab-token".to_string())
        );
    }

    #[test]
    fn test_script_auth() {
        let mut auth = make_auth("script");
        auth.command = Some("echo 'Authorization: Bearer test-from-script'".to_string());
        let headers = build_auth_headers(Some(&auth), "GET", "/test", None, None).unwrap();
        assert_eq!(
            headers.get("Authorization"),
            Some(&"Bearer test-from-script".to_string())
        );
    }

    #[test]
    fn test_script_receives_context() {
        let mut auth = make_auth("script");
        auth.command =
            Some("echo \"X-Method: $RESTIE_METHOD\"; echo \"X-Path: $RESTIE_PATH\"".to_string());
        let headers = build_auth_headers(Some(&auth), "POST", "/repos/test", None, None).unwrap();
        assert_eq!(headers.get("X-Method"), Some(&"POST".to_string()));
        assert_eq!(headers.get("X-Path"), Some(&"/repos/test".to_string()));
    }

    #[test]
    fn test_script_with_env_expansion() {
        std::env::set_var("MY_SECRET", "secret-value");
        let mut auth = make_auth("script");
        auth.command = Some("echo \"Authorization: Bearer $MY_SECRET\"".to_string());
        auth.env = Some({
            let mut m = std::collections::BTreeMap::new();
            m.insert("MY_SECRET".to_string(), "${MY_SECRET}".to_string());
            m
        });
        let headers = build_auth_headers(Some(&auth), "GET", "/test", None, None).unwrap();
        assert_eq!(
            headers.get("Authorization"),
            Some(&"Bearer secret-value".to_string())
        );
    }

    #[test]
    fn test_script_header_case_normalized() {
        // Script emits lowercase / mixed-case header names — they must be
        // canonicalized to Title-Case so lookups by standard casing work.
        let mut auth = make_auth("script");
        auth.command = Some(
            "echo 'authorization: Bearer x'; echo 'x-api-key: k'; echo 'X_CUSTOM: v'".to_string(),
        );
        let headers = build_auth_headers(Some(&auth), "GET", "/test", None, None).unwrap();
        assert_eq!(headers.get("Authorization"), Some(&"Bearer x".to_string()));
        assert_eq!(headers.get("X-Api-Key"), Some(&"k".to_string()));
        // Underscored names are a single token → first letter upper, rest lower.
        assert_eq!(headers.get("X_custom"), Some(&"v".to_string()));
        // No duplicate casing variants survive.
        assert_eq!(headers.len(), 3);
    }

    #[test]
    fn test_expand_env() {
        std::env::set_var("FOO", "bar");
        assert_eq!(expand_env("prefix-${FOO}-suffix"), "prefix-bar-suffix");
        assert_eq!(expand_env("no-vars-here"), "no-vars-here");
    }

    #[test]
    fn test_script_resolves_against_config_dir() {
        // A bare relative auth.command like `auth/aliyun-sign.sh` must resolve
        // against ~/.config/restie/ (where bundled scripts install) regardless
        // of the caller's CWD, so raw-only sites work from any directory.
        let home = std::env::temp_dir().join(format!("restie-auth-test-{}", std::process::id()));
        let auth_dir = home.join(".config/restie/auth");
        std::fs::create_dir_all(&auth_dir).unwrap();
        let script = auth_dir.join("authorizer.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\necho 'Authorization: Bearer from-config-dir'\n",
        )
        .unwrap();
        // The bundled installer writes scripts as executable; mirror that here
        // because RESTie execs the file directly rather than via a shell.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &home);
        let result = {
            let auth = AuthConfig {
                auth_type: "script".to_string(),
                token_env: None,
                header: None,
                prefix: None,
                command: Some("auth/authorizer.sh".to_string()),
                env: None,
            };
            build_auth_headers(Some(&auth), "GET", "/", None, None)
        };

        // Restore HOME before asserting, so a failing assertion can't leak a
        // mutated HOME into concurrently running tests.
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        std::fs::remove_dir_all(&home).ok();

        let headers = result.expect("script in config dir should run");
        assert_eq!(
            headers.get("Authorization"),
            Some(&"Bearer from-config-dir".to_string())
        );
    }
}
