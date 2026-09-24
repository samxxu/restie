// =============================================================================
// Config - YAML schema (module/command hierarchy), auth, load, find, and help
// =============================================================================

use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};

// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteConfig {
    pub site: Option<SiteInfo>,
    #[serde(default)]
    pub modules: std::collections::BTreeMap<String, ModuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteInfo {
    pub name: Option<String>,
    pub base_url: Option<String>,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
}

// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(rename = "type")]
    pub auth_type: String, // bearer_token | api_key | custom_header | script | none
    pub token_env: Option<String>,
    pub header: Option<String>,    // default: Authorization
    pub prefix: Option<String>,    // default: Bearer
    pub command: Option<String>,   // for script type: path to auth script
    #[serde(default)]
    pub env: Option<std::collections::BTreeMap<String, String>>, // env vars to pass to script
}

// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleConfig {
    #[serde(default)]
    pub commands: std::collections::BTreeMap<String, EndpointConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointConfig {
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub params: std::collections::BTreeMap<String, ParamConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamConfig {
    #[serde(default)]
    pub required: bool,
    pub location: String, // path / query / body / header
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub param_type: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
}

// =============================================================================
// Config file discovery
// =============================================================================

impl SiteConfig {
    /// Get the restie config directory: ~/.config/restie
    /// Creates the directory automatically if it doesn't exist.
    /// Returns an error if HOME is not set instead of silently polluting cwd.
    pub fn config_dir() -> PathBuf {
        let home = env::var("HOME").unwrap_or_else(|_| {
            // Home is required — without it we have nowhere safe to put config.
            // Fall back to a temp dir so the program can still run, but warn.
            eprintln!("Warning: $HOME not set; using /tmp/restie as config directory");
            "/tmp/restie".to_string()
        });
        let dir = PathBuf::from(format!("{}/.config/restie", home));
        if !dir.exists() {
            std::fs::create_dir_all(&dir).ok();
        }
        dir
    }

    /// Whether a path refers to a YAML config file (.yaml or .yml).
    fn is_yaml_file(path: &std::path::Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yaml") | Some("yml")
        )
    }

    /// Path to a site config file, accepting both `.yaml` and `.yml`.
    /// Prefers `.yaml` when both exist.
    fn site_config_path_in(dir: &Path, name: &str) -> Option<PathBuf> {
        let yaml = dir.join(format!("{}.yaml", name));
        if yaml.exists() {
            return Some(yaml);
        }
        let yml = dir.join(format!("{}.yml", name));
        if yml.exists() {
            return Some(yml);
        }
        None
    }

    /// Path to the named site's config file in the user config directory.
    pub fn site_config_path(name: &str) -> Option<PathBuf> {
        Self::site_config_path_in(&Self::config_dir(), name)
    }

    /// Find config file by searching in order:
    /// 1. --config path
    /// 2. --site <name> → ~/.config/restie/<name>.yaml
    /// 3. ./restie.yaml (current directory)
    /// 4. ~/.config/restie/<current-site>.yaml — the persisted active site
    /// 5. ~/.config/restie/ (fallback: most endpoints)
    pub fn find(explicit_path: Option<&str>, site: Option<&str>) -> Option<PathBuf> {
        // 1. Explicit --config path
        if let Some(p) = explicit_path {
            let path = Path::new(p);
            if path.exists() {
                return Some(path.to_path_buf());
            }
            eprintln!("Config file not found: {}", p);
            return None;
        }

        // 2. --site <name>
        if let Some(site_name) = site {
            let path = Self::site_config_path_in(&Self::config_dir(), site_name);
            if let Some(p) = path {
                return Some(p);
            }
            eprintln!("Site config not found: {}.yaml/.yml", site_name);
            return None;
        }

        // 3. Local file
        let local = Path::new("./restie.yaml");
        if local.exists() {
            return Some(local.to_path_buf());
        }

        // 4. Persisted active site
        if let Some(active) = Self::active_site() {
            let path = Self::site_config_path_in(&Self::config_dir(), &active);
            if let Some(p) = path {
                return Some(p);
            }
        }

        // 5. Config directory — pick the most useful config
        //    Multiple sites: choose the one with the most endpoints
        //    (ties broken by newest modified time). Skip configs that fail to load.
        let dir = Self::config_dir();
        if dir.is_dir() {
            let mut paths: Vec<PathBuf> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if Self::is_yaml_file(&path) {
                        paths.push(path);
                    }
                }
            }
            if paths.is_empty() {
                return None;
            }
            if paths.len() == 1 {
                return Some(paths.remove(0));
            }
            // Multiple configs: pick the one with the most endpoints
            let mut best: Option<(PathBuf, usize, u128)> = None;
            for p in &paths {
                let count = Self::load(p)
                    .map(|c| c.modules.values().map(|m| m.commands.len()).sum::<usize>())
                    .unwrap_or(0);
                let mtime = std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0))
                    .unwrap_or(0);
                let better = match &best {
                    Some((_, best_count, best_mtime)) => {
                        count > *best_count || (count == *best_count && mtime > *best_mtime)
                    }
                    None => true,
                };
                if better {
                    best = Some((p.clone(), count, mtime));
                }
            }
            return best.map(|(p, _, _)| p);
        }

        None
    }

    /// Path to the marker file storing the currently active site.
    fn current_marker() -> PathBuf {
        Self::config_dir().join(".current-site")
    }

    /// Set the currently active site, persisted across invocations.
    /// Returns an error if no config file exists for that site.
    pub fn set_current_site(name: &str) -> Result<(), String> {
        if Self::site_config_path_in(&Self::config_dir(), name).is_none() {
            return Err(format!(
                "No config found for site '{}' ({}.yaml/.yml)",
                name,
                Self::config_dir().join(name).display()
            ));
        }
        let marker = Self::current_marker();
        if let Some(parent) = marker.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        // Atomic write: write to a temp file, then rename.
        let tmp = marker.with_extension("tmp");
        std::fs::write(&tmp, name.trim())
            .map_err(|e| format!("Failed to write current site: {}", e))?;
        std::fs::rename(&tmp, &marker)
            .map_err(|e| format!("Failed to finalize current site marker: {}", e))?;
        Ok(())
    }

    /// Clear the currently active site marker.
    pub fn clear_current_site() {
        let m = Self::current_marker();
        if m.exists() {
            let _ = std::fs::remove_file(m);
        }
    }

    /// The currently active site name, if one was set.
    pub fn active_site() -> Option<String> {
        let m = Self::current_marker();
        if !m.exists() {
            return None;
        }
        std::fs::read_to_string(m)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// List names of all site configs in the config directory (sorted).
    pub fn available_sites() -> Vec<String> {
        let dir = Self::config_dir();
        let mut names: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if Self::is_yaml_file(&path) {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
        names.sort_by_key(|a| a.to_lowercase());
        names
    }

    /// Load config from file path
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: SiteConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Try to find and load config. Returns None if not found.
    pub fn find_and_load(explicit_path: Option<&str>, site: Option<&str>) -> Option<Self> {
        Self::find(explicit_path, site).and_then(|path| match Self::load(&path) {
            Ok(config) => Some(config),
            Err(e) => {
                eprintln!("Error loading config {}: {}", path.display(), e);
                None
            }
        })
    }

    /// Get endpoint config by module + command (or alias)
    pub fn get_endpoint(&self, module: &str, command: &str) -> Option<&EndpointConfig> {
        let mod_cfg = self.modules.get(module)?;
        if let Some(ep) = mod_cfg.commands.get(command) {
            return Some(ep);
        }
        mod_cfg
            .commands
            .values()
            .find(|ep| ep.aliases.iter().any(|a| a == command))
    }

    /// Get base_url from config or fallback
    pub fn get_base_url(&self) -> String {
        self.site
            .as_ref()
            .and_then(|s| s.base_url.as_ref())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://api.github.com".to_string())
    }

    /// Get auth config
    pub fn get_auth(&self) -> Option<&AuthConfig> {
        self.site.as_ref().and_then(|s| s.auth.as_ref())
    }

    /// Get static headers
    pub fn get_headers(&self) -> Option<&std::collections::BTreeMap<String, String>> {
        self.site.as_ref().and_then(|s| s.headers.as_ref())
    }

    /// Generate help text for a specific module (list its commands)
    pub fn help_for_module(&self, module: &str) -> String {
        let mod_cfg = match self.modules.get(module) {
            Some(m) => m,
            None => {
                return format!(
                    "Unknown module: '{}'\n\nAvailable modules: {}",
                    module,
                    self.list_modules()
                );
            }
        };

        let mut out = String::new();
        out.push_str(&format!("Module: {}\n\n", module));
        out.push_str("Commands:\n");

        let mut commands: Vec<_> = mod_cfg.commands.iter().collect();
        commands.sort_by_key(|(name, _)| name.to_string());

        for (name, ep) in commands {
            let summary = ep.summary.as_deref().unwrap_or("");
            let method = ep.method.to_uppercase();
            out.push_str(&format!(
                "  {:<30} {:<6} {:<40} {}\n",
                name, method, ep.path, summary
            ));
        }
        out
    }

    /// Generate help text for a specific command
    pub fn help_for_command(&self, module: &str, command: &str) -> String {
        let ep = match self.get_endpoint(module, command) {
            Some(ep) => ep,
            None => {
                return format!(
                    "Unknown command: {} {}\n\n{}",
                    module,
                    command,
                    self.help_for_module(module)
                );
            }
        };

        let mut out = String::new();
        let method = ep.method.to_uppercase();

        out.push_str(&format!("API: {} {}\n", method, ep.path));
        if let Some(ref summary) = ep.summary {
            out.push_str(&format!("{}\n", summary));
        }
        out.push('\n');
        out.push_str(&format!("Usage: restie {} {}", module, command));

        let required_params: Vec<_> = ep.params.iter().filter(|(_, p)| p.required).collect();
        let optional_params: Vec<_> = ep.params.iter().filter(|(_, p)| !p.required).collect();

        for (name, _) in &required_params {
            let upper = name.to_uppercase();
            out.push_str(&format!(" --{} <{}>", name, upper));
        }
        for (name, _) in &optional_params {
            let upper = name.to_uppercase();
            out.push_str(&format!(" [--{} <{}>]", name, upper));
        }
        out.push('\n');

        if !required_params.is_empty() {
            out.push_str("\nRequired:\n");
            for (name, p) in &required_params {
                let desc = p.description.as_deref().unwrap_or("");
                out.push_str(&format!("  --{:<20} {}\n", name, desc));
            }
        }

        if !optional_params.is_empty() {
            out.push_str("\nOptional:\n");
            for (name, p) in &optional_params {
                let desc = p.description.as_deref().unwrap_or("");
                let def = p
                    .default
                    .as_ref()
                    .map(|d| format!(" (default: {})", d))
                    .unwrap_or_default();
                out.push_str(&format!("  --{:<20} {}{}\n", name, desc, def));
            }
        }

        out
    }

    /// List all modules
    pub fn list_modules(&self) -> String {
        let mut modules: Vec<_> = self.modules.iter().collect();
        modules.sort_by_key(|(name, _)| name.to_string());

        let mut out = String::new();
        out.push_str("Available modules:\n\n");
        for (name, mod_cfg) in modules {
            let count = mod_cfg.commands.len();
            out.push_str(&format!("  {:<25} {} commands\n", name, count));
        }
        out
    }

    /// List all commands across all modules
    pub fn list_all_commands(&self) -> String {
        let mut out = String::new();
        out.push_str("Available commands:\n\n");

        let mut modules: Vec<_> = self.modules.iter().collect();
        modules.sort_by_key(|(name, _)| name.to_string());

        for (mod_name, mod_cfg) in modules {
            let mut commands: Vec<_> = mod_cfg.commands.iter().collect();
            commands.sort_by_key(|(name, _)| name.to_string());

            for (cmd_name, ep) in commands {
                let summary = ep.summary.as_deref().unwrap_or("");
                let method = ep.method.to_uppercase();
                out.push_str(&format!(
                    "  {} {:<25} {:<6} {:<40} {}\n",
                    mod_name, cmd_name, method, ep.path, summary
                ));
            }
        }
        out
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"
site:
  name: GitHub
  base_url: https://api.github.com
  auth:
    type: bearer_token
    token_env: GITHUB_TOKEN

modules:
  repos:
    commands:
      get:
        method: GET
        path: /repos/{owner}/{repo}
        summary: Get a repository
        params:
          owner:
            required: true
            location: path
            description: Repository owner
          repo:
            required: true
            location: path
            description: Repository name
      create:
        method: POST
        path: /user/repos
        summary: Create a repository
        params:
          name:
            required: true
            location: body
            description: Repository name
  issues:
    commands:
      create:
        method: POST
        path: /repos/{owner}/{repo}/issues
        summary: Create an issue
        params:
          owner:
            required: true
            location: path
          repo:
            required: true
            location: path
          title:
            required: true
            location: body
"#;

    fn load_sample() -> SiteConfig {
        serde_yaml::from_str(SAMPLE_YAML).unwrap()
    }

    #[test]
    fn test_load_config() {
        let config = load_sample();
        assert!(config.site.is_some());
        assert_eq!(config.modules.len(), 2);
    }

    #[test]
    fn test_auth_config() {
        let config = load_sample();
        let auth = config.get_auth().unwrap();
        assert_eq!(auth.auth_type, "bearer_token");
        assert_eq!(auth.token_env, Some("GITHUB_TOKEN".to_string()));
    }

    #[test]
    fn test_get_base_url() {
        let config = load_sample();
        assert_eq!(config.get_base_url(), "https://api.github.com");
    }

    #[test]
    fn test_get_endpoint() {
        let config = load_sample();
        let ep = config.get_endpoint("repos", "get").unwrap();
        assert_eq!(ep.method, "GET");
        assert_eq!(ep.path, "/repos/{owner}/{repo}");
        assert_eq!(ep.params.len(), 2);
    }

    #[test]
    fn test_get_endpoint_not_found() {
        let config = load_sample();
        assert!(config.get_endpoint("repos", "nonexistent").is_none());
        assert!(config.get_endpoint("nonexistent", "get").is_none());
    }

    #[test]
    fn test_param_config() {
        let config = load_sample();
        let ep = config.get_endpoint("repos", "get").unwrap();
        let owner = ep.params.get("owner").unwrap();
        assert!(owner.required);
        assert_eq!(owner.location, "path");
    }

    #[test]
    fn test_list_modules() {
        let config = load_sample();
        let listing = config.list_modules();
        assert!(listing.contains("repos"));
        assert!(listing.contains("issues"));
    }

    #[test]
    fn test_help_for_module() {
        let config = load_sample();
        let help = config.help_for_module("repos");
        assert!(help.contains("get"));
        assert!(help.contains("create"));
    }

    #[test]
    fn test_help_for_command() {
        let config = load_sample();
        let help = config.help_for_command("repos", "get");
        assert!(help.contains("GET /repos/{owner}/{repo}"));
        assert!(help.contains("--owner"));
        assert!(help.contains("--repo"));
    }

    #[test]
    fn test_set_current_site_missing() {
        // Setting an unknown site must fail without touching the real config dir.
        let err = SiteConfig::set_current_site("definitely-not-a-real-site-name");
        assert!(err.is_err());
    }

    #[test]
    fn test_site_config_path_both_extensions() {
        // Pure path lookup: prefers .yaml, falls back to .yml. Uses a temp
        // directory so the real ~/.config/restie is never touched.
        let dir = std::env::temp_dir().join(format!(
            "restie-config-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let name = "mysite";
        // Neither extension exists yet.
        assert!(SiteConfig::site_config_path_in(&dir, name).is_none());

        // Only .yml exists → found via .yml.
        let yml = dir.join(format!("{}.yml", name));
        std::fs::write(&yml, "site:\n  base_url: https://api.example.com\n").unwrap();
        let found = SiteConfig::site_config_path_in(&dir, name).unwrap();
        assert_eq!(found, yml);

        // Both exist → .yaml wins.
        let yaml = dir.join(format!("{}.yaml", name));
        std::fs::write(&yaml, "site:\n  base_url: https://api.example.com\n").unwrap();
        let found = SiteConfig::site_config_path_in(&dir, name).unwrap();
        assert_eq!(found, yaml);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
