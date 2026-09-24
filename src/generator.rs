// =============================================================================
// Generator - OpenAPI spec to YAML config converter (module/command hierarchy)
// =============================================================================

use crate::config::{EndpointConfig, ModuleConfig, ParamConfig, SiteConfig, SiteInfo};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Parse an OpenAPI spec (JSON or YAML string) and generate a SiteConfig
/// organized into modules (from OpenAPI tags) and commands (from summary).
pub fn generate_config(
    spec_content: &str,
    filter: Option<&str>,
) -> Result<SiteConfig, Box<dyn std::error::Error>> {
    let spec: Value = if spec_content.trim_start().starts_with('{') {
        serde_json::from_str(spec_content)?
    } else {
        serde_yaml::from_str(spec_content)?
    };

    let base_url = spec["servers"][0]["url"]
        .as_str()
        .unwrap_or_else(|| {
            eprintln!(
                "Warning: no server URL found in OpenAPI spec, falling back to https://api.github.com"
            );
            "https://api.github.com"
        })
        .trim_end_matches('/')
        .to_string();

    let site_name = spec["info"]["title"]
        .as_str()
        .unwrap_or_else(|| {
            eprintln!("Warning: no info.title found in OpenAPI spec, using 'API'");
            "API"
        })
        .to_string();

    // modules: tag_name → BTreeMap<command_name, EndpointConfig>
    let mut modules: BTreeMap<String, ModuleConfig> = BTreeMap::new();

    if let Some(paths) = spec["paths"].as_object() {
        for (path, path_item) in paths {
            if let Some(filter) = filter {
                if !path.contains(filter) {
                    continue;
                }
            }

            for (method, operation) in path_item.as_object().unwrap_or(&Map::new()) {
                let method_upper = method.to_uppercase();
                if !matches!(
                    method_upper.as_str(),
                    "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
                ) {
                    continue;
                }

                // Extract module name from tags (first tag), fallback to "misc"
                let module_name = operation["tags"]
                    .as_array()
                    .and_then(|tags| tags.first())
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "misc".to_string());

                let operation_id = operation["operationId"]
                    .as_str()
                    .map(|s| s.to_string());

                let summary = operation["summary"]
                    .as_str()
                    .map(|s| s.to_string());

                // Primary command name from summary, stripping the module word.
                // Fallback (computed once and reused for aliases) is derived
                // from method + path when neither summary nor operationId exist.
                let fallback = derive_fallback_name(&method_upper, path);
                let primary_name = summary
                    .as_ref()
                    .map(|s| summary_to_command_name(s, &module_name))
                    .unwrap_or_else(|| {
                        operation_id
                            .clone()
                            .map(|id| strip_module_prefix(&id, &module_name))
                            .unwrap_or_else(|| fallback.clone())
                    });

                // Build aliases: operationId + fallback name
                let mut aliases: Vec<String> = Vec::new();
                if let Some(ref op_id) = operation_id {
                    if *op_id != primary_name {
                        aliases.push(op_id.clone());
                    }
                }
                if fallback != primary_name && !aliases.contains(&fallback) {
                    aliases.push(fallback);
                }

                // Build params
                let mut params: BTreeMap<String, ParamConfig> = BTreeMap::new();

                if let Some(op_params) = operation["parameters"].as_array() {
                    extract_params(op_params, &mut params, &spec);
                }
                if let Some(path_params) = path_item["parameters"].as_array() {
                    extract_params(path_params, &mut params, &spec);
                }

                extract_body_params(&operation["requestBody"], &mut params, &spec);

                let endpoint = EndpointConfig {
                    method: method_upper,
                    path: path.clone(),
                    summary,
                    aliases,
                    params,
                };

                // Insert into module, handle name collisions
                let mod_cfg = modules.entry(module_name.clone()).or_insert_with(|| {
                    ModuleConfig {
                        commands: BTreeMap::new(),
                    }
                });

                let mut final_name = primary_name.clone();
                let mut counter = 2;
                while mod_cfg.commands.contains_key(&final_name) {
                    final_name = format!("{}-{}", primary_name, counter);
                    counter += 1;
                }

                mod_cfg.commands.insert(final_name, endpoint);
            }
        }
    }

    Ok(SiteConfig {
        site: Some(SiteInfo {
            name: Some(site_name),
            base_url: Some(base_url),
            auth: None,
            headers: None,
        }),
        modules,
    })
}

/// Convert API summary to a short command name, stripping the module word.
/// e.g. "Get a repository" with module "repos" → "get"
///      "List repositories for the authenticated user" with module "repos" → "list-mine"
///      "Create an issue" with module "issues" → "create"
fn summary_to_command_name(summary: &str, module: &str) -> String {
    // Lowercase and replace non-alphanumeric with dashes
    let mut name: String = summary
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    // Remove "Closed - " prefix
    if name.starts_with("closed---") {
        name = name.trim_start_matches("closed---").to_string();
    }

    // Collapse multiple dashes
    while name.contains("--") {
        name = name.replace("--", "-");
    }

    let name = name.trim_matches('-').to_string();

    let mut words: Vec<&str> = name.split('-').filter(|w| !w.is_empty()).collect();

    // Articles to remove
    let articles = ["a", "an", "the"];

    // Check for "for the authenticated user" / "by the authenticated user" suffix
    let for_auth_user = ["for", "the", "authenticated", "user"];
    let by_auth_user = ["by", "the", "authenticated", "user"];

    let has_my = if words.len() >= 4 {
        let suffix = &words[words.len() - 4..];
        suffix == for_auth_user || suffix == by_auth_user
    } else {
        false
    };

    if has_my {
        words.truncate(words.len() - 4);
    }

    // Remove articles
    let words: Vec<&str> = words.iter().filter(|w| !articles.contains(w)).copied().collect();

    // Simplify nouns: singular/plural forms
    let words: Vec<String> = words
        .iter()
        .map(|w| match *w {
            "repositories" => "repos",
            "repository" => "repo",
            "organizations" => "orgs",
            "organization" => "org",
            other => other,
        })
        .map(|s| s.to_string())
        .collect();

    // Strip module word(s) and their variants from the command name.
    // Module "repos" should strip: repos, repo, repositories, repository
    // Module "issues" should strip: issues, issue
    let module_variants: Vec<String> = module
        .split('-')
        .flat_map(word_variants)
        .collect();
    let words: Vec<String> = words
        .iter()
        .filter(|w| !module_variants.iter().any(|mv| mv.as_str() == w.as_str()))
        .cloned()
        .collect();

    // If "my" concept, insert "mine" after the verb (first word)
    // "list mine" = "list my repos" — stands alone without a noun
    let result = if has_my && !words.is_empty() {
        let mut result = vec![words[0].clone(), "mine".to_string()];
        result.extend(words.iter().skip(1).cloned());
        result.join("-")
    } else {
        words.join("-")
    };

    let result = result.replace("--", "-").trim_matches('-').to_string();

    // Don't return empty string — fall back to first word of summary
    if result.is_empty() {
        summary
            .split_whitespace()
            .next()
            .map(|w| w.to_lowercase())
            .unwrap_or("command".to_string())
    } else {
        result
    }
}

/// Generate word variants for a module name.
/// "repos" → ["repos", "repo", "repositories", "repository"]
/// "issues" → ["issues", "issue"]
/// "orgs" → ["orgs", "org", "organizations", "organization"]
fn word_variants(word: &str) -> Vec<String> {
    let mut variants = vec![word.to_string()];
    // Remove trailing 's' for singular
    if let Some(singular) = word.strip_suffix('s') {
        variants.push(singular.to_string());
    }
    // Common expansions
    match word {
        "repos" | "repo" => {
            variants.push("repositories".to_string());
            variants.push("repository".to_string());
        }
        "orgs" | "org" => {
            variants.push("organizations".to_string());
            variants.push("organization".to_string());
        }
        _ => {}
    }
    variants
}

/// Strip module prefix from operationId.
/// e.g. "repos/get" with module "repos" → "get"
fn strip_module_prefix(operation_id: &str, module: &str) -> String {
    let prefix = format!("{}/", module);
    if operation_id.starts_with(&prefix) {
        operation_id[prefix.len()..].to_string()
    } else {
        operation_id.to_string()
    }
}

/// Fallback: derive command name from method+path when no operationId.
fn derive_fallback_name(method: &str, path: &str) -> String {
    let path_clean = path
        .trim_start_matches('/')
        .replace(['{', '}'], "");
    format!("{}-{}", method.to_lowercase(), path_clean.replace('/', "-"))
}

/// Extract parameters from OpenAPI parameters array.
/// Handles $ref references to #/components/parameters/{name}.
fn extract_params(
    params_array: &[Value],
    params: &mut BTreeMap<String, ParamConfig>,
    spec: &Value,
) {
    for param in params_array {
        let param = if let Some(ref_path) = param["$ref"].as_str() {
            resolve_ref(spec, ref_path)
        } else {
            param.clone()
        };

        let name = match param["name"].as_str() {
            Some(n) => n.to_string(),
            None => {
                if param["$ref"].is_string() || param.is_null() {
                    eprintln!(
                        "Warning: skipping parameter with unresolvable $ref ({})",
                        param["$ref"].as_str().unwrap_or("null")
                    );
                }
                continue;
            }
        };
        if params.contains_key(&name) {
            continue;
        }

        let location = param["in"].as_str().unwrap_or("query").to_string();
        let required = param["required"].as_bool().unwrap_or(false);
        let description = param["description"]
            .as_str()
            .map(|s| s.to_string());
        // Resolve schema-level $ref so type/default survive component references.
        let schema = if let Some(r) = param["schema"]["$ref"].as_str() {
            resolve_ref(spec, r)
        } else {
            param["schema"].clone()
        };
        let param_type = schema["type"].as_str().map(|s| s.to_string());
        let default = default_to_string(&schema["default"]);

        params.insert(
            name,
            ParamConfig {
                required,
                location,
                description,
                param_type,
                default,
            },
        );
    }
}

/// Resolve a $ref pointer like "#/components/parameters/owner" in the spec.
/// Recursively follows nested $ref pointers so callers always get a concrete value.
/// Returns `Value::Null` if any segment along the chain is missing or if a
/// circular reference is detected (guards against malformed specs that would
/// otherwise recurse forever).
fn resolve_ref(spec: &Value, ref_path: &str) -> Value {
    resolve_ref_depth(spec, ref_path, 0)
}

const MAX_REF_DEPTH: usize = 32;

fn resolve_ref_depth(spec: &Value, ref_path: &str, depth: usize) -> Value {
    if depth > MAX_REF_DEPTH {
        eprintln!(
            "Warning: circular or excessively deep $ref chain at '{}' (depth {}); stopping",
            ref_path, depth
        );
        return Value::Null;
    }
    let path = ref_path.trim_start_matches("#/");
    let mut current = spec;
    for segment in path.split('/') {
        match current.get(segment) {
            Some(next) => current = next,
            None => return Value::Null,
        }
    }
    // If the resolved value is itself a $ref, keep chasing (bounded).
    if let Some(next_ref) = current["$ref"].as_str() {
        resolve_ref_depth(spec, next_ref, depth + 1)
    } else {
        current.clone()
    }
}

/// Extract body parameters from requestBody.
/// Handles schemas with `$ref`, and falls back to the first available
/// content-type when `application/json` is not present.
fn extract_body_params(
    request_body: &Value,
    params: &mut BTreeMap<String, ParamConfig>,
    spec: &Value,
) {
    let content = match request_body["content"].as_object() {
        Some(c) => c,
        None => return,
    };

    // Prefer application/json; otherwise fall back to the first content-type
    // we can find (e.g. form-urlencoded, multipart/form-data) so params are
    // not silently dropped for non-JSON APIs.
    let schema_value = if let Some(json_ct) = content.get("application/json") {
        &json_ct["schema"]
    } else if let Some((_, first_ct)) = content.iter().next() {
        &first_ct["schema"]
    } else {
        return;
    };

    // Resolve $ref at the schema level.
    let schema = if let Some(r) = schema_value["$ref"].as_str() {
        resolve_ref(spec, r)
    } else {
        schema_value.clone()
    };

    let props = match schema["properties"].as_object() {
        Some(p) => p,
        None => return,
    };

    let required_fields: Vec<&str> = schema["required"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect()
        })
        .unwrap_or_default();

    for (name, prop_schema) in props {
        if params.contains_key(name) {
            continue;
        }
        // Resolve per-property $ref so we get the real type/default/description.
        let prop = if let Some(r) = prop_schema["$ref"].as_str() {
            resolve_ref(spec, r)
        } else {
            prop_schema.clone()
        };

        params.insert(
            name.clone(),
            ParamConfig {
                required: required_fields.contains(&name.as_str()),
                location: "body".to_string(),
                description: prop["description"].as_str().map(|s| s.to_string()),
                param_type: prop["type"].as_str().map(|s| s.to_string()),
                default: default_to_string(&prop["default"]),
            },
        );
    }
}

/// Convert a JSON default value to its string representation for the config.
fn default_to_string(val: &Value) -> Option<String> {
    match val {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(arr) => Some(serde_json::to_string(arr).unwrap_or_default()),
        Value::Object(obj) => Some(serde_json::to_string(obj).unwrap_or_default()),
        Value::Null => None,
    }
}

/// Serialize SiteConfig to YAML string.
pub fn to_yaml(config: &SiteConfig) -> Result<String, Box<dyn std::error::Error>> {
    let yaml = serde_yaml::to_string(config)?;
    Ok(yaml)
}

/// Derive a filename from the OpenAPI spec's info.title.
/// "GitHub v3 REST API" → "github"
/// "Vultr API v2" → "vultr"
/// "Amazon EC2 API" → "amazon-ec2"
pub fn site_name_to_filename(title: &str) -> String {
    let lower = title.to_lowercase();

    // Remove common suffixes
    let stripped = lower
        .replace("rest api", "")
        .replace("api", "")
        .replace("v3", "")
        .replace("v2", "")
        .replace("v1", "")
        .trim()
        .to_string();

    // Replace non-alphanumeric with dashes, collapse multiple dashes
    let name: String = stripped
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    let name = name.trim_matches('-').to_string();

    // Collapse multiple dashes
    let mut collapsed = name;
    while collapsed.contains("--") {
        collapsed = collapsed.replace("--", "-");
    }

    // Take only the first meaningful word(s) — e.g. "amazon-ec2" stays, "github-" → "github"
    let collapsed = collapsed.trim_matches('-').to_string();

    if collapsed.is_empty() {
        // Nothing survived the cleanup (e.g. "V3 REST API v2" → "").
        // Fall back to the first alphanumeric word of the raw title instead
        // of a generic "site", so the filename stays meaningful.
        title
            .split(|c: char| !c.is_ascii_alphanumeric())
            .find(|w| !w.is_empty())
            .map(|w| w.to_lowercase())
            .unwrap_or_else(|| "site".to_string())
    } else {
        collapsed
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_OPENAPI: &str = r#"{
  "openapi": "3.0.0",
  "info": { "title": "GitHub API", "version": "1.0" },
  "servers": [{ "url": "https://api.github.com" }],
  "paths": {
    "/repos/{owner}/{repo}": {
      "get": {
        "operationId": "repos/get",
        "summary": "Get a repository",
        "tags": ["repos"],
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "description": "Repository owner", "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "description": "Repository name", "schema": { "type": "string" } }
        ]
      },
      "delete": {
        "operationId": "repos/delete",
        "summary": "Delete a repository",
        "tags": ["repos"],
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } }
        ]
      }
    },
    "/user/repos": {
      "get": {
        "operationId": "repos/list-for-authenticated-user",
        "summary": "List repositories for the authenticated user",
        "tags": ["repos"],
        "parameters": [
          { "name": "per_page", "in": "query", "required": false, "description": "Per page", "schema": { "type": "integer", "default": 30 } }
        ]
      },
      "post": {
        "operationId": "repos/create-for-authenticated-user",
        "summary": "Create a repository for the authenticated user",
        "tags": ["repos"],
        "requestBody": {
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "required": ["name"],
                "properties": {
                  "name": { "type": "string", "description": "Repository name" },
                  "description": { "type": "string", "description": "Repository description" }
                }
              }
            }
          }
        }
      }
    },
    "/repos/{owner}/{repo}/issues": {
      "post": {
        "operationId": "issues/create",
        "summary": "Create an issue",
        "tags": ["issues"],
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } }
        ],
        "requestBody": {
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "required": ["title"],
                "properties": {
                  "title": { "type": "string", "description": "Issue title" },
                  "body": { "type": "string", "description": "Issue body" }
                }
              }
            }
          }
        }
      }
    }
  }
}"#;

    #[test]
    fn test_generate_config_basic() {
        let config = generate_config(SAMPLE_OPENAPI, None).unwrap();
        assert!(config.site.is_some());
        let site = config.site.unwrap();
        assert_eq!(site.name, Some("GitHub API".to_string()));
        assert_eq!(site.base_url, Some("https://api.github.com".to_string()));
    }

    #[test]
    fn test_generate_config_modules() {
        let config = generate_config(SAMPLE_OPENAPI, None).unwrap();
        assert_eq!(config.modules.len(), 2);
        assert!(config.modules.contains_key("repos"));
        assert!(config.modules.contains_key("issues"));
    }

    #[test]
    fn test_command_names_stripped_of_module_word() {
        let config = generate_config(SAMPLE_OPENAPI, None).unwrap();
        let repos = config.modules.get("repos").unwrap();

        // "Get a repository" → strip "repository" → "get"
        assert!(repos.commands.contains_key("get"));
        // "Delete a repository" → strip "repository" → "delete"
        assert!(repos.commands.contains_key("delete"));
        // "List repositories for the authenticated user" → strip "repos" + "for the authenticated user" → "list-mine"
        assert!(repos.commands.contains_key("list-mine"));
        // "Create a repository for the authenticated user" → "create-mine"
        assert!(repos.commands.contains_key("create-mine"));
    }

    #[test]
    fn test_operation_id_aliases_preserved() {
        let config = generate_config(SAMPLE_OPENAPI, None).unwrap();
        let repos = config.modules.get("repos").unwrap();

        let get_ep = repos.commands.get("get").unwrap();
        assert!(get_ep.aliases.contains(&"repos/get".to_string()));

        let create_ep = repos.commands.get("create-mine").unwrap();
        assert!(create_ep.aliases.contains(&"repos/create-for-authenticated-user".to_string()));
    }

    #[test]
    fn test_path_params() {
        let config = generate_config(SAMPLE_OPENAPI, None).unwrap();
        let repos = config.modules.get("repos").unwrap();
        let ep = repos.commands.get("get").unwrap();
        assert_eq!(ep.method, "GET");
        assert_eq!(ep.path, "/repos/{owner}/{repo}");

        let owner = ep.params.get("owner").unwrap();
        assert!(owner.required);
        assert_eq!(owner.location, "path");
        assert_eq!(owner.description, Some("Repository owner".to_string()));
    }

    #[test]
    fn test_body_params() {
        let config = generate_config(SAMPLE_OPENAPI, None).unwrap();
        let issues = config.modules.get("issues").unwrap();
        let ep = issues.commands.get("create").unwrap();
        assert_eq!(ep.method, "POST");

        let title = ep.params.get("title").unwrap();
        assert!(title.required);
        assert_eq!(title.location, "body");
    }

    #[test]
    fn test_filter() {
        let config = generate_config(SAMPLE_OPENAPI, Some("issues")).unwrap();
        // Only paths containing "issues" pass the filter
        let repos_count = config.modules.get("repos").map(|m| m.commands.len()).unwrap_or(0);
        assert_eq!(repos_count, 0);
        let issues_count = config.modules.get("issues").map(|m| m.commands.len()).unwrap_or(0);
        assert_eq!(issues_count, 1);
    }

    #[test]
    fn test_to_yaml() {
        let config = generate_config(SAMPLE_OPENAPI, None).unwrap();
        let yaml = to_yaml(&config).unwrap();
        assert!(yaml.contains("modules:"));
        assert!(yaml.contains("repos:"));
        assert!(yaml.contains("issues:"));
        assert!(yaml.contains("method: GET"));
        assert!(yaml.contains("location: path"));
        assert!(yaml.contains("location: body"));
    }

    #[test]
    fn test_site_name_to_filename() {
        assert_eq!(site_name_to_filename("GitHub v3 REST API"), "github");
        assert_eq!(site_name_to_filename("Vultr API v2"), "vultr");
        assert_eq!(site_name_to_filename("Amazon EC2 API"), "amazon-ec2");
        assert_eq!(site_name_to_filename("Aliyun API"), "aliyun");
    }

    #[test]
    fn test_site_name_to_filename_empty_after_strip() {
        // "V3 REST API v2" strips to "" — falls back to the first raw word.
        assert_eq!(site_name_to_filename("V3 REST API v2"), "v3");
        // Title with no usable content at all still yields a sane name.
        assert_eq!(site_name_to_filename("API v2"), "api");
        assert_eq!(site_name_to_filename("---"), "site");
    }

    #[test]
    fn test_body_params_with_schema_ref() {
        let spec = r##"{
          "openapi": "3.0.0",
          "info": { "title": "Test" },
          "servers": [{ "url": "https://api.example.com" }],
          "components": {
            "schemas": {
              "Repo": {
                "type": "object",
                "required": ["name"],
                "properties": {
                  "name": { "type": "string", "description": "Repo name" },
                  "private": { "type": "boolean", "default": false }
                }
              }
            }
          },
          "paths": {
            "/user/repos": {
              "post": {
                "operationId": "repos/create",
                "summary": "Create a repository",
                "tags": ["repos"],
                "requestBody": {
                  "content": {
                    "application/json": {
                      "schema": { "$ref": "#/components/schemas/Repo" }
                    }
                  }
                }
              }
            }
          }
        }"##;
        let config = generate_config(spec, None).unwrap();
        let module = &config.modules["repos"];
        let cmd = module.commands.values().next().unwrap();
        assert_eq!(cmd.params["name"].location, "body");
        assert!(cmd.params["name"].required);
        assert_eq!(cmd.params["private"].default.as_deref(), Some("false"));
    }

    #[test]
    fn test_body_params_form_urlencoded_fallback() {
        let spec = r#"{
          "openapi": "3.0.0",
          "info": { "title": "Test" },
          "servers": [{ "url": "https://api.example.com" }],
          "paths": {
            "/login": {
              "post": {
                "operationId": "auth/login",
                "summary": "Login with credentials",
                "tags": ["auth"],
                "requestBody": {
                  "content": {
                    "application/x-www-form-urlencoded": {
                      "schema": {
                        "type": "object",
                        "required": ["username"],
                        "properties": {
                          "username": { "type": "string" },
                          "password": { "type": "string" }
                        }
                      }
                    }
                  }
                }
              }
            }
          }
        }"#;
        let config = generate_config(spec, None).unwrap();
        let module = &config.modules["auth"];
        let cmd = module.commands.values().next().unwrap();
        assert_eq!(cmd.params.len(), 2);
        assert_eq!(cmd.params["username"].location, "body");
        assert!(cmd.params["username"].required);
    }

    #[test]
    fn test_default_value_types() {
        let spec = r#"{
          "openapi": "3.0.0",
          "info": { "title": "Test" },
          "servers": [{ "url": "https://api.example.com" }],
          "paths": {
            "/search": {
              "get": {
                "operationId": "search/get",
                "summary": "Search items",
                "tags": ["search"],
                "parameters": [
                  { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 20 } },
                  { "name": "ratio", "in": "query", "schema": { "type": "number", "default": 1.5 } },
                  { "name": "active", "in": "query", "schema": { "type": "boolean", "default": true } },
                  { "name": "tags", "in": "query", "schema": { "type": "array", "default": ["a", "b"] } }
                ]
              }
            }
          }
        }"#;
        let config = generate_config(spec, None).unwrap();
        let module = &config.modules["search"];
        let cmd = module.commands.values().next().unwrap();
        assert_eq!(cmd.params["limit"].default.as_deref(), Some("20"));
        assert_eq!(cmd.params["ratio"].default.as_deref(), Some("1.5"));
        assert_eq!(cmd.params["active"].default.as_deref(), Some("true"));
        assert_eq!(cmd.params["tags"].default.as_deref(), Some("[\"a\",\"b\"]"));
    }

    #[test]
    fn test_circular_ref_terminates() {
        // p → q → p forms a $ref cycle. Resolution must terminate (bounded by
        // MAX_REF_DEPTH) and the broken param must be skipped, not crash.
        let spec = r##"{
          "openapi": "3.0.0",
          "info": { "title": "Test" },
          "servers": [{ "url": "https://api.example.com" }],
          "components": {
            "parameters": {
              "p": { "$ref": "#/components/parameters/q" },
              "q": { "$ref": "#/components/parameters/p" }
            }
          },
          "paths": {
            "/items/{id}": {
              "get": {
                "operationId": "items/get",
                "summary": "Get an item",
                "tags": ["items"],
                "parameters": [
                  { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } },
                  { "$ref": "#/components/parameters/p" }
                ]
              }
            }
          }
        }"##;
        let config = generate_config(spec, None).unwrap();
        let module = &config.modules["items"];
        let cmd = module.commands.values().next().unwrap();
        // The cyclic ref must have been dropped; the well-formed `id` param survives.
        assert_eq!(cmd.params.len(), 1);
        assert!(cmd.params.contains_key("id"));
    }

    #[test]
    fn test_ref_chain_to_concrete_value() {
        // Multi-hop ref: param schema A → B → concrete schema. Type/default
        // must be resolved through the whole chain.
        let spec = r##"{
          "openapi": "3.0.0",
          "info": { "title": "Test" },
          "servers": [{ "url": "https://api.example.com" }],
          "components": {
            "schemas": {
              "A": { "$ref": "#/components/schemas/B" },
              "B": { "type": "integer", "default": 7 }
            }
          },
          "paths": {
            "/things": {
              "get": {
                "operationId": "things/get",
                "summary": "List things",
                "tags": ["things"],
                "parameters": [
                  { "name": "limit", "in": "query", "schema": { "$ref": "#/components/schemas/A" } }
                ]
              }
            }
          }
        }"##;
        let config = generate_config(spec, None).unwrap();
        let module = &config.modules["things"];
        let cmd = module.commands.values().next().unwrap();
        assert_eq!(cmd.params["limit"].param_type.as_deref(), Some("integer"));
        assert_eq!(cmd.params["limit"].default.as_deref(), Some("7"));
    }
}
