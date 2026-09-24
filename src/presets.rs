// =============================================================================
// Presets - platform preset catalog
//
// The preset list is DATA, not code, and it does not live in this repository.
// The authoritative catalog lives in the separate presets repository
// (https://github.com/samxxu/restie-presets), so platforms can be added,
// re-pointed or removed without a RESTie release. This module is only the
// mechanism: it fetches the catalog, validates it, caches it, resolves it and
// hands it to the generator.
//
// Nothing platform-specific is compiled into the binary. On a fresh machine the
// cache is empty, so the first run that needs presets triggers a one-time fetch
// (see `bootstrap_if_needed`).
//
// Resolution order:
//   1. local override  ~/.config/restie/presets/catalog.local.yaml
//   2. remote cache    ~/.config/restie/presets/catalog.yaml
// =============================================================================

use crate::config::AuthConfig;
use crate::http_client::USER_AGENT;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Newest catalog schema this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Oldest catalog schema this build still understands. Raise this only when a
/// change makes older catalogs unusable, never for additive changes.
pub const MIN_SCHEMA_VERSION: u32 = 1;

/// Default presets repository (raw-file base URL, no trailing slash).
///
/// The catalog is just files in a git repo, fetched over raw.githubusercontent
/// so RESTie needs no git binary and no credentials. Overridable per invocation
/// with `restie presets update --repo <url>` or globally with the
/// `RESTIE_PRESETS_REPO` environment variable.
pub const DEFAULT_PRESETS_REPO: &str = "https://raw.githubusercontent.com/samxxu/restie-presets/main";

// -----------------------------------------------------------------------------
// Catalog schema
// -----------------------------------------------------------------------------

/// One platform preset. The `default_auth` field is spelled `auth` in YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    /// Short id used on the command line, e.g. `github`. Must be unique.
    pub name: String,
    /// Human-readable title for the platform (e.g. "GitHub REST API"), as opposed
    /// to `name`, which is what you type on the command line. Exposed to callers
    /// via `preset_list()`.
    pub display_name: String,
    /// OpenAPI spec location fetched by `restie generate <name>`. Empty for
    /// raw-only (script-auth) presets, which have no usable spec; those carry a
    /// `base_url` instead.
    #[serde(default)]
    pub openapi_url: String,
    /// Default request base URL for raw-only (script-auth) presets that have
    /// no usable spec — used as site.base_url in the generated config.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Auth defaults injected into the generated site config.
    #[serde(rename = "auth", default)]
    pub default_auth: Option<AuthConfig>,
    /// One-line summary shown in `restie presets`.
    pub description: String,
}

/// A whole catalog file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetCatalog {
    /// Format version of the catalog file.
    pub schema_version: u32,
    /// Repo-managed signing scripts to download alongside the catalog. The
    /// bundled snapshot ships none; the presets repository lists them here so
    /// `presets update` knows every file to fetch.
    #[serde(default)]
    pub scripts: Vec<String>,
    #[serde(default)]
    pub presets: Vec<Preset>,
}

// -----------------------------------------------------------------------------
// Parsing and validation
// -----------------------------------------------------------------------------

/// Parse and validate a catalog from YAML text.
///
/// This runs on every catalog load, not only on the bundled snapshot: an
/// updated catalog fetched from the network is untrusted input, and a version
/// mismatch must surface as a clear error instead of a silently wrong preset.
pub fn parse_catalog(content: &str) -> Result<PresetCatalog, String> {
    let catalog: PresetCatalog =
        serde_yaml::from_str(content).map_err(|e| format!("invalid preset catalog: {}", e))?;

    if catalog.schema_version < MIN_SCHEMA_VERSION || catalog.schema_version > SCHEMA_VERSION {
        return Err(format!(
            "unsupported preset catalog schema_version {} (this build supports {}..={})",
            catalog.schema_version, MIN_SCHEMA_VERSION, SCHEMA_VERSION
        ));
    }

    let mut seen = BTreeSet::new();
    for p in &catalog.presets {
        if p.name.trim().is_empty() {
            return Err("preset with an empty 'name'".to_string());
        }
        if !seen.insert(p.name.as_str()) {
            return Err(format!("duplicate preset name '{}'", p.name));
        }
        if p.display_name.trim().is_empty() {
            return Err(format!("preset '{}' has an empty 'display_name'", p.name));
        }
        if p.description.trim().is_empty() {
            return Err(format!("preset '{}' has an empty 'description'", p.name));
        }
        // Auth invariants first: a broken auth block gives a more specific
        // error than a missing URL, so report it before the URL rules below.
        if let Some(auth) = &p.default_auth {
            validate_auth(&p.name, auth)?;
        }
        // The catalog now comes entirely from a repo, so these completeness
        // checks are the only thing standing between a bad entry and a confusing
        // failure at request time.
        //
        // A raw-only preset (script auth) has no usable OpenAPI spec, so what it
        // needs instead is the API's own base URL. A spec-based preset needs the
        // spec location. Requiring the wrong one is what we are catching here.
        if is_script_auth(p) {
            match p.base_url.as_deref() {
                Some(b) if is_http_url(b) => {}
                Some(b) => {
                    return Err(format!(
                        "preset '{}' base_url must be an http(s) URL, got '{}'",
                        p.name, b
                    ))
                }
                None => {
                    return Err(format!(
                        "preset '{}' uses script auth but has no 'base_url' \
                         (raw-only presets need the API host)",
                        p.name
                    ))
                }
            }
        } else if !is_http_url(&p.openapi_url) {
            return Err(format!(
                "preset '{}' openapi_url must be a non-empty http(s) URL, got '{}'",
                p.name, p.openapi_url
            ));
        }
        if p.openapi_url.contains(char::is_whitespace) {
            return Err(format!(
                "preset '{}' openapi_url must not contain whitespace",
                p.name
            ));
        }
    }

    for script in &catalog.scripts {
        validate_script_path("scripts manifest", script)?;
    }

    Ok(catalog)
}

/// Whether this preset is raw-only: its auth is provided by a signing script,
/// which also means it has no usable OpenAPI spec.
fn is_script_auth(p: &Preset) -> bool {
    p.default_auth
        .as_ref()
        .map(|a| a.auth_type == "script")
        .unwrap_or(false)
}

/// Whether a string is a non-empty http(s) URL.
fn is_http_url(s: &str) -> bool {
    !s.trim().is_empty() && (s.starts_with("http://") || s.starts_with("https://"))
}

/// Reject absolute paths and `..` escapes in any script path coming from a
/// catalog, so an updated (untrusted) catalog can never point RESTie at an
/// arbitrary location on disk.
fn validate_script_path(owner: &str, command: &str) -> Result<(), String> {
    let path = Path::new(command);
    let escapes = path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        });
    if command.trim().is_empty() || escapes {
        return Err(format!(
            "{} has an unsafe script path '{}' (must be relative, no '..')",
            owner, command
        ));
    }
    Ok(())
}

/// Guard rails for auth entries that came from a catalog file.
///
/// A catalog can name an environment variable but must never carry its value,
/// and it must never be able to point RESTie at an arbitrary executable.
fn validate_auth(preset: &str, auth: &AuthConfig) -> Result<(), String> {
    if auth.auth_type == "script" {
        let command = auth.command.as_deref().ok_or_else(|| {
            format!(
                "preset '{}' uses script auth but has no 'command'",
                preset
            )
        })?;
        validate_script_path(preset, command)?;
    }

    if let Some(env) = &auth.env {
        if env.keys().any(|k| k.trim().is_empty()) {
            return Err(format!("preset '{}' has an empty env var name", preset));
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Catalog sources (two-layer resolution)
// -----------------------------------------------------------------------------

/// An empty catalog, used before the first successful fetch.
pub fn empty_catalog() -> PresetCatalog {
    PresetCatalog {
        schema_version: SCHEMA_VERSION,
        scripts: Vec::new(),
        presets: Vec::new(),
    }
}

/// Where the effective catalog came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    /// No cached catalog on disk yet (nothing has been fetched). The effective
    /// catalog is then empty unless a local override supplies presets.
    Empty,
    /// A catalog fetched by `restie presets update` and cached locally.
    Remote,
}

/// The catalog currently in effect as loaded once per process.
pub struct Effective {
    /// Base catalog with any local override already merged in.
    pub catalog: PresetCatalog,
    /// Where the *base* (pre-override) catalog came from.
    pub source: CatalogSource,
    /// How many presets the local override contributed (replaced or added).
    pub local_overrides: usize,
    pub description: String,
}

/// Best catalog for this run, resolved in two layers (highest first):
///
///   1. **local override** — `~/.config/restie/presets/catalog.local.yaml`.
///      Hand-written and never written to by `restie presets update`, so your
///      own presets survive every refresh.
///   2. **remote cache** — `~/.config/restie/presets/catalog.yaml`, staged by
///      `restie presets update` (or the first-run bootstrap).
///
/// The remote cache is the base; the local override is merged on top of it,
/// replacing same-named presets and appending new ones. With neither present the
/// catalog is simply empty, and `restie` tells the user to fetch it.
pub fn effective() -> &'static Effective {
    static EFF: OnceLock<Effective> = OnceLock::new();
    EFF.get_or_init(|| {
        let (base, source, base_desc) = match load_cached_catalog() {
            Some((catalog, path)) => (
                catalog,
                CatalogSource::Remote,
                format!("remote cache ({})", path.display()),
            ),
            None => (
                empty_catalog(),
                CatalogSource::Empty,
                "no catalog fetched yet".to_string(),
            ),
        };

        match load_local_override() {
            Some(overlay) => {
                let catalog = merge_catalog(&base, &overlay);
                Effective {
                    local_overrides: overlay.presets.len(),
                    catalog,
                    source,
                    description: format!(
                        "{} + {} preset(s) from local override ({})",
                        base_desc,
                        overlay.presets.len(),
                        local_override_path().display()
                    ),
                }
            }
            None => Effective {
                catalog: base,
                source,
                local_overrides: 0,
                description: base_desc,
            },
        }
    })
}

/// Catalog used for lookups / listings this run.
pub fn effective_catalog() -> &'static PresetCatalog {
    &effective().catalog
}

/// Whether the *base* catalog has been fetched yet, or is still empty.
pub fn effective_source() -> CatalogSource {
    effective().source
}

/// How many presets the local override contributed this run (0 = none).
pub fn effective_local_overrides() -> usize {
    effective().local_overrides
}

/// Human-readable description of the current catalog source.
pub fn effective_description() -> &'static str {
    effective().description.as_str()
}

// -----------------------------------------------------------------------------
// Catalog accessors (against the effective catalog)
// -----------------------------------------------------------------------------

/// All presets in the effective catalog.
pub fn all_presets() -> Vec<Preset> {
    effective_catalog().presets.clone()
}

/// Look up a preset by name in the effective catalog.
pub fn find_preset(name: &str) -> Option<Preset> {
    find_preset_in(effective_catalog(), name)
}

/// Get a BTreeMap of preset name → display name for listing.
pub fn preset_list() -> BTreeMap<String, String> {
    preset_list_in(effective_catalog())
}

/// Lookup against an explicit catalog. Split out from `find_preset` so the
/// behaviour is testable against a fixture, since the effective catalog is
/// empty until something is fetched.
fn find_preset_in(catalog: &PresetCatalog, name: &str) -> Option<Preset> {
    catalog
        .presets
        .iter()
        .find(|p| p.name.as_str() == name)
        .cloned()
}

fn preset_list_in(catalog: &PresetCatalog) -> BTreeMap<String, String> {
    catalog
        .presets
        .iter()
        .map(|p| (p.name.clone(), p.display_name.clone()))
        .collect()
}

// -----------------------------------------------------------------------------
// Remote preset cache
// -----------------------------------------------------------------------------

/// The presets cache directory: ~/.config/restie/presets/
pub fn cache_dir() -> PathBuf {
    crate::config::SiteConfig::config_dir().join("presets")
}

/// Path to the cached (updated) catalog: ~/.config/restie/presets/catalog.yaml
pub fn cache_catalog_path() -> PathBuf {
    cache_dir().join("catalog.yaml")
}

/// Path to the user-editable local override catalog:
/// ~/.config/restie/presets/catalog.local.yaml
///
/// Deliberately a *separate file* from the remote cache: `presets update` only
/// ever writes `catalog.yaml`, so hand-written presets here survive every
/// refresh and always win over the fetched catalog.
pub fn local_override_path() -> PathBuf {
    cache_dir().join("catalog.local.yaml")
}

/// Load and validate a catalog file, warning (and degrading) on bad input so a
/// typo in a hand-edited catalog never bricks `restie generate`.
fn load_catalog_file(path: &Path) -> Option<PresetCatalog> {
    let content = std::fs::read_to_string(path).ok()?;
    match parse_catalog(&content) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!(
                "Warning: ignoring invalid presets catalog {}: {}",
                path.display(),
                e
            );
            None
        }
    }
}

fn load_cached_catalog() -> Option<(PresetCatalog, PathBuf)> {
    let path = cache_catalog_path();
    load_catalog_file(&path).map(|c| (c, path))
}

fn load_local_override() -> Option<PresetCatalog> {
    load_catalog_file(&local_override_path())
}

/// Merge a local-override catalog on top of a base catalog (layer 1 over 2/3).
///
/// Same-named presets are replaced wholesale (the override wins), new names are
/// appended, and the two `scripts:` manifests are unioned so `presets update`
/// keeps fetching any signing script the override relies on. Existing order is
/// preserved so `restie presets` output stays stable across runs.
fn merge_catalog(base: &PresetCatalog, overlay: &PresetCatalog) -> PresetCatalog {
    let mut presets: Vec<Preset> = Vec::with_capacity(base.presets.len() + overlay.presets.len());
    for p in &base.presets {
        match overlay.presets.iter().find(|o| o.name == p.name) {
            Some(o) => presets.push(o.clone()),
            None => presets.push(p.clone()),
        }
    }
    for o in &overlay.presets {
        if !base.presets.iter().any(|p| p.name == o.name) {
            presets.push(o.clone());
        }
    }

    let mut scripts: BTreeSet<String> = base.scripts.iter().cloned().collect();
    scripts.extend(overlay.scripts.iter().cloned());

    PresetCatalog {
        schema_version: base.schema_version.max(overlay.schema_version),
        scripts: scripts.into_iter().collect(),
        presets,
    }
}

// -----------------------------------------------------------------------------
// Repository update
// -----------------------------------------------------------------------------

/// Files a catalog needs RESTie to have on disk: every signing script named by
/// a script-auth preset plus the catalog's own `scripts:` manifest.
fn required_script_paths(catalog: &PresetCatalog) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for preset in &catalog.presets {
        if let Some(auth) = &preset.default_auth {
            if auth.auth_type == "script" {
                if let Some(command) = &auth.command {
                    paths.insert(command.clone());
                }
            }
        }
    }
    for script in &catalog.scripts {
        paths.insert(script.clone());
    }
    paths
}

/// Summary of a successful `presets update`.
#[derive(Debug)]
pub struct UpdateSummary {
    pub preset_count: usize,
    pub script_count: usize,
    pub catalog_url: String,
}

/// Resolve the presets repository base URL: --repo > RESTIE_PRESETS_REPO > built-in default.
fn resolve_repo_base(repo: Option<&str>) -> Result<String, String> {
    if let Some(t) = repo.map(str::trim) {
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    if let Ok(raw) = std::env::var("RESTIE_PRESETS_REPO") {
        let t = raw.trim();
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    if !DEFAULT_PRESETS_REPO.trim().is_empty() {
        return Ok(DEFAULT_PRESETS_REPO.trim().to_string());
    }
    Err(
        "presets repository URL is not configured.\n\
         Pass `restie presets update --repo <raw-base-url>` or set the \
         RESTIE_PRESETS_REPO environment variable."
            .to_string(),
    )
}

/// The repository that `presets update` (and the first-run bootstrap) would use
/// right now: `RESTIE_PRESETS_REPO` if set, otherwise the built-in default.
///
/// Public so the first-run path can say where it is about to reach out *before*
/// it does, instead of leaving a slow or blocked network looking like a hang.
pub fn default_repo_base() -> Result<String, String> {
    resolve_repo_base(None)
}

/// Fetch the latest catalog + signing scripts from the presets repository and
/// stage them into ~/.config/restie/presets/ with atomic writes. Everything is
/// downloaded (and the catalog validated) before anything is committed, so a
/// partial fetch never leaves a half-updated cache.
pub async fn update_from_repo(repo: Option<&str>) -> Result<UpdateSummary, String> {
    let base = resolve_repo_base(repo)?;
    let base = base.trim_end_matches('/').to_string();
    let (catalog, catalog_yml, staged) = download_repo(&base).await?;
    let cache_dir = crate::config::SiteConfig::config_dir().join("presets");
    stage_catalog(&cache_dir, &catalog_yml, &staged)?;
    Ok(UpdateSummary {
        preset_count: catalog.presets.len(),
        script_count: staged.len(),
        catalog_url: format!("{}/catalog.yaml", base),
    })
}

/// Whether a *usable* cached catalog (layer 2) is already on disk. An unreadable
/// or invalid cache counts as absent, so the bootstrap can repair it.
pub fn cached_catalog_exists() -> bool {
    load_cached_catalog().is_some()
}

/// Outcome of the first-run bootstrap.
#[derive(Debug)]
pub enum Bootstrap {
    /// A usable cached catalog was already present. Nothing was fetched and no
    /// network request was made.
    AlreadyCached,
    /// No catalog was cached, so one was just fetched from the default repo.
    Fetched(UpdateSummary),
    /// No catalog was cached and the fetch failed.
    Failed(String),
}

/// First-run bootstrap: if no usable catalog has ever been cached, fetch one from
/// the default presets repository, so a fresh install has presets without the
/// user having to know `restie presets update` exists.
///
/// Deliberately cheap and non-fatal: once a cache exists this does one file read
/// and no network I/O, which is why it is safe to call on every presets-using
/// command. A failed fetch leaves the cache empty rather than erroring out; the
/// caller decides how loudly to report it.
pub async fn bootstrap_if_needed() -> Bootstrap {
    if cached_catalog_exists() {
        return Bootstrap::AlreadyCached;
    }
    match update_from_repo(None).await {
        Ok(summary) => Bootstrap::Fetched(summary),
        Err(e) => Bootstrap::Failed(e),
    }
}

/// Download and validate everything, without touching disk. Broken catalogs and
/// missing scripts fail here so nothing is ever half-committed.
async fn download_repo(
    base: &str,
) -> Result<(PresetCatalog, String, Vec<(String, String)>), String> {
    // Bounded: the catalog is small, so a stall means a broken connection rather
    // than a slow link, and hanging for minutes would be worse than failing.
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))?;

    let catalog_url = format!("{}/catalog.yaml", base);
    let catalog_yml = fetch_raw(&client, &catalog_url).await?;
    let catalog = parse_catalog(&catalog_yml)
        .map_err(|e| format!("downloaded catalog is invalid (not applied): {}", e))?;

    let mut staged: Vec<(String, String)> = Vec::new();
    for rel in required_script_paths(&catalog) {
        let url = format!("{}/{}", base, rel);
        let content = fetch_raw(&client, &url).await?;
        staged.push((rel, content));
    }
    Ok((catalog, catalog_yml, staged))
}

/// Commit a fully-downloaded repo into a cache dir using atomic writes.
///
/// Only the fetched files (`catalog.yaml` plus the repo's scripts) are written.
/// In particular `catalog.local.yaml` — the local override layer — is never
/// touched here, so a refresh cannot clobber hand-written presets.
fn stage_catalog(
    cache_dir: &Path,
    catalog_yml: &str,
    staged: &[(String, String)],
) -> Result<(), String> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| format!("failed to create presets cache dir: {}", e))?;
    write_atomic(&cache_dir.join("catalog.yaml"), catalog_yml)
        .map_err(|e| format!("failed to write catalog: {}", e))?;
    for (rel, content) in staged {
        let dest = cache_dir.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create script dir: {}", e))?;
        }
        write_atomic(&dest, content).map_err(|e| format!("failed to write {}: {}", rel, e))?;
    }
    Ok(())
}

async fn fetch_raw(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| describe_network_error(url, &e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} from {}", resp.status(), url));
    }
    resp.text()
        .await
        .map_err(|e| describe_body_error(url, &e))
}

/// Body-level counterpart to `describe_network_error`. A transfer that starts but
/// never finishes is common on a flaky link, and reqwest's "error decoding
/// response body" gives no hint that retrying is the fix.
fn describe_body_error(url: &str, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return format!("timed out reading the response body from {}", url);
    }
    if e.is_body() || e.is_decode() {
        return format!(
            "the transfer from {} was interrupted before the body finished\n  \
             Retry: the catalog is small, so this is usually a flaky link rather \
             than the host being down.",
            url
        );
    }
    format!("could not read the response body from {}: {}", url, e)
}

/// Turn a reqwest failure into something actionable. reqwest's own Display embeds
/// the URL (which we print anyway) and "error sending request" says nothing about
/// what to check, which is unhelpful for the most common failures.
fn describe_network_error(url: &str, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return format!(
            "timed out fetching {}\n  The host is slow or unreachable from here.",
            url
        );
    }
    if e.is_connect() {
        return format!(
            "could not connect to {}\n  Check your network, or a proxy RESTie cannot use \
             (HTTPS_PROXY / ALL_PROXY). Both http(s):// and socks5h:// proxies work.",
            url
        );
    }
    format!("failed to fetch {}: {}", url, e)
}

/// Write via a temp file + atomic rename so a crash never leaves a truncated file.
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Core ships no preset data
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_repo_points_at_the_presets_repository() {
        // The catalog is data living in a separate repo. Pin the URL so a typo
        // (wrong user, wrong branch) cannot silently break every fresh install.
        assert_eq!(
            DEFAULT_PRESETS_REPO,
            "https://raw.githubusercontent.com/samxxu/restie-presets/main"
        );
        assert!(
            !DEFAULT_PRESETS_REPO.ends_with('/'),
            "the raw base URL must not have a trailing slash"
        );
    }

    #[test]
    fn test_empty_catalog_is_empty_and_well_formed() {
        let c = empty_catalog();
        assert!(c.presets.is_empty());
        assert!(c.scripts.is_empty());
        assert_eq!(c.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn test_core_repo_ships_no_catalog_or_scripts() {
        // Guards the whole point of the decoupling: the core repository must not
        // carry preset data. The catalog and the signing scripts live in the
        // presets repo; if either reappears here, this fails.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(
            !root.join("presets/catalog.yaml").exists(),
            "preset data must not live in the core repo"
        );
        let auth_dir = root.join("auth");
        if auth_dir.is_dir() {
            let leftovers: Vec<String> = std::fs::read_dir(&auth_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                // Ignore dot-files: editors and the OS drop .DS_Store & friends
                // into any directory that has existed, which says nothing about
                // whether preset data was shipped.
                .filter(|name| !name.starts_with('.'))
                .collect();
            assert!(
                leftovers.is_empty(),
                "signing scripts must not live in the core repo, found: {:?}",
                leftovers
            );
        }
    }

    // -------------------------------------------------------------------------
    // Schema version handling
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_catalog_rejects_newer_schema() {
        let yaml = "schema_version: 99\npresets: []\n";
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("schema_version"), "got: {}", err);
        assert!(err.contains("99"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_older_schema_below_min() {
        let yaml = "schema_version: 0\npresets: []\n";
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("schema_version"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_requires_schema_version() {
        // Missing schema_version must be a hard error, not a silent default.
        let err = parse_catalog("presets: []\n").unwrap_err();
        assert!(err.contains("invalid preset catalog"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_minimal_valid() {
        let catalog = parse_catalog("schema_version: 1\npresets: []\n").unwrap();
        assert_eq!(catalog.schema_version, 1);
        assert!(catalog.presets.is_empty());
    }

    #[test]
    fn test_parse_catalog_rejects_malformed_yaml() {
        let err = parse_catalog("schema_version: 1\npresets: [oops\n").unwrap_err();
        assert!(err.contains("invalid preset catalog"), "got: {}", err);
    }

    // -------------------------------------------------------------------------
    // Entry validation
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_catalog_rejects_duplicate_names() {
        let yaml = r#"
schema_version: 1
presets:
  - name: dup
    display_name: First
    openapi_url: https://example.com/a.json
    description: first
  - name: dup
    display_name: Second
    openapi_url: https://example.com/b.json
    description: second
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("duplicate"), "got: {}", err);
        assert!(err.contains("dup"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_empty_name() {
        let yaml = r#"
schema_version: 1
presets:
  - name: ""
    display_name: Nameless
    openapi_url: https://example.com/a.json
    description: nameless
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("empty 'name'"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_empty_openapi_url() {
        let yaml = r#"
schema_version: 1
presets:
  - name: nousl
    display_name: No URL
    openapi_url: ""
    description: missing url
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("openapi_url"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_unknown_field() {
        // Typo'd keys must fail loudly rather than silently dropping config.
        let yaml = r#"
schema_version: 1
presets:
  - name: typo
    display_name: Typo
    openapi_urln: https://example.com/a.json
    description: misspelled openapi_url
"#;
        assert!(parse_catalog(yaml).is_err());
    }

    #[test]
    fn test_parse_catalog_rejects_script_without_command() {
        let yaml = r#"
schema_version: 1
presets:
  - name: noscript
    display_name: No Script
    openapi_url: https://example.com/a.json
    description: script auth without command
    auth:
      type: script
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("command"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_absolute_script_path() {
        let yaml = r#"
schema_version: 1
presets:
  - name: evil
    display_name: Evil
    openapi_url: https://example.com/a.json
    description: absolute path
    auth:
      type: script
      command: /tmp/evil.sh
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("unsafe script path"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_parent_dir_script_path() {
        let yaml = r#"
schema_version: 1
presets:
  - name: escape
    display_name: Escape
    openapi_url: https://example.com/a.json
    description: parent dir escape
    auth:
      type: script
      command: ../../tmp/evil.sh
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("unsafe script path"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_accepts_relative_script_path() {
        // A raw-only preset: script auth, a safe relative command, and a
        // base_url (required instead of an OpenAPI spec).
        let yaml = r#"
schema_version: 1
presets:
  - name: good
    display_name: Good
    base_url: https://good.example.com
    description: fine
    auth:
      type: script
      command: auth/good-sign.sh
      env:
        GOOD_KEY: ${GOOD_KEY}
"#;
        let catalog = parse_catalog(yaml).unwrap();
        let auth = catalog.presets[0].default_auth.as_ref().unwrap();
        assert_eq!(auth.command.as_deref(), Some("auth/good-sign.sh"));
        assert_eq!(catalog.presets[0].openapi_url, "");
    }

    #[test]
    fn test_parse_catalog_rejects_script_preset_without_base_url() {
        // Raw-only presets have no spec to fetch, so the API host is the thing
        // that must be present; without it `generate` would have nothing to aim at.
        let yaml = r#"
schema_version: 1
presets:
  - name: hostless
    display_name: Hostless
    description: script auth but no base_url
    auth:
      type: script
      command: auth/x.sh
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("base_url"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_script_preset_with_bad_base_url() {
        let yaml = r#"
schema_version: 1
presets:
  - name: badhost
    display_name: Bad Host
    base_url: example.com
    description: base_url is not a URL
    auth:
      type: script
      command: auth/x.sh
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("base_url must be an http(s) URL"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_allows_spec_preset_without_base_url() {
        // The mirror case: a spec-based preset needs no base_url, since
        // `generate` derives the host from the fetched spec.
        let yaml = r#"
schema_version: 1
presets:
  - name: spec
    display_name: Spec Based
    openapi_url: https://example.com/openapi.json
    description: no base_url needed
    auth:
      type: bearer_token
      token_env: SPEC_TOKEN
"#;
        assert!(parse_catalog(yaml).is_ok());
    }

    #[test]
    fn test_parse_catalog_rejects_empty_env_name() {
        let yaml = r#"
schema_version: 1
presets:
  - name: emptyenv
    display_name: Empty Env
    openapi_url: https://example.com/a.json
    description: empty env key
    auth:
      type: script
      command: auth/x.sh
      env:
        "": value
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("env var name"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_unsafe_manifest_script() {
        // The `scripts:` manifest is also path-checked: an updated (untrusted)
        // catalog must not be able to direct RESTie to an arbitrary absolute or
        // escaping path.
        let yaml = r#"
schema_version: 1
scripts:
  - ../evil.sh
presets: []
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("unsafe script path"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_accepts_manifest_with_scripts() {
        let yaml = r#"
schema_version: 1
scripts:
  - auth/aliyun-sign.sh
  - auth/tencent-sign.sh
presets: []
"#;
        let catalog = parse_catalog(yaml).unwrap();
        assert_eq!(catalog.scripts.len(), 2);
        assert!(catalog.scripts.contains(&"auth/aliyun-sign.sh".to_string()));
    }

    #[test]
    fn test_parse_catalog_rejects_non_http_openapi_url() {
        // The catalog is now the only source of truth, so a relative or malformed
        // spec location must be caught at parse time, not at request time.
        let yaml = r#"
schema_version: 1
presets:
  - name: bad
    display_name: Bad
    openapi_url: ./local/spec.json
    description: not a URL
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("http(s) URL"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_empty_display_name() {
        let yaml = r#"
schema_version: 1
presets:
  - name: noname
    display_name: "  "
    openapi_url: https://example.com/a.json
    description: has a description
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("display_name"), "got: {}", err);
    }

    #[test]
    fn test_parse_catalog_rejects_empty_description() {
        let yaml = r#"
schema_version: 1
presets:
  - name: nodesc
    display_name: No Description
    openapi_url: https://example.com/a.json
    description: ""
"#;
        let err = parse_catalog(yaml).unwrap_err();
        assert!(err.contains("description"), "got: {}", err);
    }

    // -------------------------------------------------------------------------
    // Lookup behaviour
    //
    // Everything here runs against a fixture, because the effective catalog is
    // empty until a fetch happens. Core has no presets of its own to look up.
    // -------------------------------------------------------------------------

    /// Stands in for whatever the presets repo currently ships.
    const LOOKUP_FIXTURE: &str = r#"
schema_version: 1
presets:
  - name: github
    display_name: GitHub REST API
    openapi_url: https://api.github.com/openapi.json
    description: fixture github
    auth:
      type: bearer_token
      token_env: GITHUB_TOKEN
  - name: gitlab
    display_name: GitLab API v4
    openapi_url: https://gitlab.com/openapi.yaml
    description: fixture gitlab
    auth:
      type: custom_header
      token_env: GITLAB_TOKEN
      header: PRIVATE-TOKEN
"#;

    #[test]
    fn test_find_preset_in_fixture() {
        let catalog = parse_catalog(LOOKUP_FIXTURE).unwrap();
        let p = find_preset_in(&catalog, "github").unwrap();
        assert_eq!(p.name, "github");
        assert!(p.default_auth.is_some());
    }

    #[test]
    fn test_find_preset_missing_is_none() {
        let catalog = parse_catalog(LOOKUP_FIXTURE).unwrap();
        assert!(find_preset_in(&catalog, "nonexistent").is_none());
    }

    #[test]
    fn test_find_preset_in_empty_catalog_is_none() {
        // With nothing fetched there are no presets, and core must not invent
        // any. This is the "no catalog yet" state.
        assert!(find_preset_in(&empty_catalog(), "github").is_none());
    }

    #[test]
    fn test_preset_list_in_fixture() {
        let catalog = parse_catalog(LOOKUP_FIXTURE).unwrap();
        let list = preset_list_in(&catalog);
        assert_eq!(list.len(), 2);
        assert_eq!(list.get("github").map(String::as_str), Some("GitHub REST API"));
        assert_eq!(list.get("gitlab").map(String::as_str), Some("GitLab API v4"));
    }

    #[test]
    fn test_preset_list_of_empty_catalog_is_empty() {
        assert!(preset_list_in(&empty_catalog()).is_empty());
    }

    #[test]
    fn test_github_preset_has_bearer_auth() {
        let catalog = parse_catalog(LOOKUP_FIXTURE).unwrap();
        let auth = find_preset_in(&catalog, "github").unwrap().default_auth.unwrap();
        assert_eq!(auth.auth_type, "bearer_token");
        assert_eq!(auth.token_env, Some("GITHUB_TOKEN".to_string()));
    }

    #[test]
    fn test_gitlab_preset_has_custom_header() {
        let catalog = parse_catalog(LOOKUP_FIXTURE).unwrap();
        let auth = find_preset_in(&catalog, "gitlab").unwrap().default_auth.unwrap();
        assert_eq!(auth.auth_type, "custom_header");
        assert_eq!(auth.header, Some("PRIVATE-TOKEN".to_string()));
    }

    #[test]
    fn test_aliyun_preset_has_script_auth() {
        let p = repo_preset("aliyun");
        let auth = p.default_auth.unwrap();
        assert_eq!(auth.auth_type, "script");
        assert_eq!(auth.command.as_deref(), Some("auth/aliyun-sign.sh"));
        assert!(auth.env.is_some());
        let env = auth.env.unwrap();
        assert!(env.contains_key("ALIYUN_ACCESS_KEY_ID"));
        assert!(env.contains_key("ALIYUN_ACCESS_KEY_SECRET"));
    }

    #[test]
    fn test_tencent_preset_has_script_auth() {
        let p = repo_preset("tencent");
        let auth = p.default_auth.unwrap();
        assert_eq!(auth.auth_type, "script");
        assert_eq!(auth.command.as_deref(), Some("auth/tencent-sign.sh"));
        assert!(auth.env.is_some());
        let env = auth.env.unwrap();
        assert!(env.contains_key("TENCENTCLOUD_SECRET_ID"));
        assert!(env.contains_key("TENCENTCLOUD_SECRET_KEY"));
        assert!(env.contains_key("TENCENTCLOUD_SERVICE"));
        assert!(env.contains_key("TENCENTCLOUD_REGION"));
    }

    #[test]
    fn test_aliyun_env_values_are_indirections_not_secrets() {
        // A catalog may only name env vars; values must be ${VAR} references
        // resolved from the user's own environment, never literals.
        let auth = repo_preset("aliyun").default_auth.unwrap();
        for (key, value) in auth.env.unwrap() {
            assert!(
                value.starts_with("${") && value.ends_with('}'),
                "{} should be a ${{VAR}} indirection, got '{}'",
                key,
                value
            );
        }
    }

    #[test]
    fn test_raw_only_presets_have_base_urls() {
        // Script-auth presets have no usable OpenAPI spec, so they must ship a
        // base_url for raw mode — otherwise the generated config would default
        // to a generic host, which is wrong for Aliyun/Tencent.
        let catalog = parse_catalog(REPO_CATALOG).unwrap();
        for name in ["aliyun", "tencent"] {
            let p = catalog
                .presets
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("preset '{}' missing", name));
            let auth = p
                .default_auth
                .as_ref()
                .unwrap_or_else(|| panic!("preset '{}' lost its auth", name));
            assert_eq!(auth.auth_type, "script", "preset '{}' must be script-auth", name);
            let base = p
                .base_url
                .as_deref()
                .unwrap_or_else(|| panic!("preset '{}' must define a base_url", name));
            assert!(base.starts_with("https://"), "preset '{}' base_url: {}", name, base);
        }
    }

    // -------------------------------------------------------------------------
    // Remote cache + repository update
    // -------------------------------------------------------------------------

    #[test]
    fn test_required_script_paths_from_command_and_manifest() {
        let catalog = parse_catalog(REPO_CATALOG).unwrap();
        let paths = required_script_paths(&catalog);
        // From script-auth commands plus the explicit `scripts:` manifest.
        assert!(paths.contains("auth/aliyun-sign.sh"));
        assert!(paths.contains("auth/tencent-sign.sh"));
    }

    /// Serializes the tests that mutate `RESTIE_PRESETS_REPO`, which is
    /// process-wide while Rust's test runner executes tests in parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_resolve_repo_base_prefers_explicit_arg() {
        let _guard = env_guard();
        std::env::set_var("RESTIE_PRESETS_REPO", "https://env.example/repo");
        let got = resolve_repo_base(Some("https://arg.example/repo")).unwrap();
        assert_eq!(got, "https://arg.example/repo");
        std::env::remove_var("RESTIE_PRESETS_REPO");
    }

    #[test]
    fn test_resolve_repo_base_falls_back_to_env() {
        let _guard = env_guard();
        std::env::set_var("RESTIE_PRESETS_REPO", "https://env.example/repo");
        let got = resolve_repo_base(Some("   ")).unwrap();
        assert_eq!(got, "https://env.example/repo");
        std::env::remove_var("RESTIE_PRESETS_REPO");
    }

    #[test]
    fn test_resolve_repo_base_falls_back_to_builtin_default() {
        // With no --repo and no env override, the built-in default (the official
        // presets repo) applies, so `presets update` and the first-run bootstrap
        // work with zero configuration.
        let _guard = env_guard();
        std::env::remove_var("RESTIE_PRESETS_REPO");
        assert_eq!(resolve_repo_base(None).unwrap(), DEFAULT_PRESETS_REPO);
    }

    #[tokio::test]
    async fn test_download_repo_fetches_catalog_and_scripts() {
        // Serves a tiny in-process HTTP repo and verifies download_repo pulls
        // the catalog + manifest scripts into memory (no HOME / config dir used).
        let catalog = r#"
schema_version: 1
scripts:
  - auth/aliyun-sign.sh
presets:
  - name: aliyun
    display_name: Alibaba Cloud
    openapi_url: https://example.invalid/a.json
    base_url: https://ecs.aliyuncs.com
    description: fixture
    auth:
      type: script
      command: auth/aliyun-sign.sh
      env:
        ALIYUN_ACCESS_KEY_ID: ${ALIBABA_CLOUD_ACCESS_KEY_ID}
"#;
        let files = vec![
            ("/catalog.yaml", catalog),
            ("/auth/aliyun-sign.sh", "#!/bin/bash\necho 'X-Test: ok'\n"),
        ];
        let base = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || serve_fixture_repo(listener, files));
            format!("http://{}", addr)
        };

        let (catalog, catalog_yml, staged) = download_repo(&base)
            .await
            .expect("download_repo should succeed against fixture repo");
        assert_eq!(catalog.presets.len(), 1);
        assert!(catalog_yml.contains("schema_version: 1"));
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].0, "auth/aliyun-sign.sh");
        assert!(staged[0].1.contains("X-Test"));
    }

    #[tokio::test]
    async fn test_download_repo_rejects_invalid_catalog() {
        // A catalog that fails validation must abort the download with an error.
        let files = vec![
            ("/catalog.yaml", "schema_version: 999\npresets: []\n"),
            ("/auth/evil.sh", "#!/bin/bash\n"),
        ];
        let base = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || serve_fixture_repo(listener, files));
            format!("http://{}", addr)
        };
        let err = download_repo(&base).await.unwrap_err();
        assert!(err.contains("invalid"), "got: {}", err);
    }

    #[tokio::test]
    async fn test_download_repo_rejects_missing_script() {
        // If a script named by the catalog is missing, nothing should be staged.
        let files = vec![
            ("/catalog.yaml", REPO_CATALOG),
            // deliberately no /auth/aliyun-sign.sh
            ("/auth/tencent-sign.sh", "#!/bin/bash\n"),
        ];
        let base = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || serve_fixture_repo(listener, files));
            format!("http://{}", addr)
        };
        assert!(download_repo(&base).await.is_err());
    }

    #[test]
    fn test_stage_catalog_writes_files_atomically() {
        let dir = std::env::temp_dir().join(format!("restie-stage-test-{}", std::process::id()));
        let cached = dir.join("presets");
        let staged = vec![
            ("auth/aliyun-sign.sh".to_string(), "script-one\n".to_string()),
            ("auth/tencent-sign.sh".to_string(), "script-two\n".to_string()),
        ];
        stage_catalog(&cached, "schema_version: 1\npresets: []\n", &staged).unwrap();

        assert!(cached.join("catalog.yaml").is_file());
        assert!(cached.join("auth").is_dir()); // nested dir auto-created
        assert_eq!(
            std::fs::read_to_string(cached.join("auth/aliyun-sign.sh")).unwrap(),
            "script-one\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // -------------------------------------------------------------------------
    // Local override layer (layer 1 of the three-layer resolution)
    // -------------------------------------------------------------------------

    #[test]
    fn test_local_override_path_is_not_the_remote_cache() {
        // The local override MUST be a distinct file, otherwise `presets update`
        // would overwrite hand-written presets and there would be no layer 1.
        assert_ne!(local_override_path(), cache_catalog_path());
        assert!(
            local_override_path().ends_with("presets/catalog.local.yaml"),
            "got {}",
            local_override_path().display()
        );
    }

    #[test]
    fn test_merge_catalog_overrides_by_name_and_appends_new() {
        let base = parse_catalog(CATALOG_WITH_TWO).unwrap();
        let overlay = parse_catalog(OVERRIDE_CATALOG).unwrap();

        let merged = merge_catalog(&base, &overlay);
        assert_eq!(merged.presets.len(), 3, "2 base + 1 new, 1 replaced");

        // Order preserved: base order first, brand-new names appended.
        let names: Vec<&str> = merged.presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["github", "stripe", "internal"]);

        // The override replaced the base entry wholesale.
        assert_eq!(merged.presets[0].display_name, "GitHub (patched)");
        assert_eq!(merged.presets[0].openapi_url, "https://example.com/gh-v2.json");

        // An untouched base entry survives verbatim.
        assert_eq!(merged.presets[1].description, "base stripe");
        assert_eq!(
            merged.presets[2].default_auth.as_ref().unwrap().auth_type,
            "bearer_token"
        );
    }

    #[test]
    fn test_merge_catalog_does_not_mutate_inputs() {
        let base = parse_catalog(CATALOG_WITH_TWO).unwrap();
        let overlay = parse_catalog(OVERRIDE_CATALOG).unwrap();
        let merged = merge_catalog(&base, &overlay);

        assert_eq!(merged.presets.len(), 3);
        assert_eq!(base.presets.len(), 2, "base must be untouched");
        assert_eq!(overlay.presets.len(), 2, "overlay must be untouched");
        assert_eq!(base.presets[0].display_name, "GitHub");
    }

    #[test]
    fn test_merge_catalog_unions_and_sorts_scripts() {
        let base =
            parse_catalog("schema_version: 1\nscripts:\n  - auth/a.sh\n  - auth/shared.sh\npresets: []\n")
                .unwrap();
        let overlay =
            parse_catalog("schema_version: 1\nscripts:\n  - auth/shared.sh\n  - auth/b.sh\npresets: []\n")
                .unwrap();

        let merged = merge_catalog(&base, &overlay);
        // Union, deduplicated and sorted — a script named by either layer is
        // still fetched by `presets update`.
        assert_eq!(merged.scripts, vec!["auth/a.sh", "auth/b.sh", "auth/shared.sh"]);
    }

    #[test]
    fn test_merge_catalog_takes_highest_schema_version() {
        let base = parse_catalog("schema_version: 1\npresets: []\n").unwrap();
        let overlay = parse_catalog("schema_version: 1\npresets: []\n").unwrap();
        assert_eq!(merge_catalog(&base, &overlay).schema_version, 1);
    }

    #[test]
    fn test_local_override_is_validated_like_any_catalog() {
        // A hand-edited override gets the same guard rails as a fetched one: a
        // bad schema version or an unsafe script path is rejected, not honored.
        let dir = std::env::temp_dir().join(format!("restie-override-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let bad = dir.join("catalog.local.yaml");
        std::fs::write(&bad, "schema_version: 99\npresets: []\n").unwrap();
        assert!(load_catalog_file(&bad).is_none(), "newer schema must be rejected");

        std::fs::write(
            &bad,
            "schema_version: 1\npresets:\n  - name: evil\n    display_name: E\n    openapi_url: https://e.com/a.json\n    description: d\n    auth:\n      type: script\n      command: /tmp/evil.sh\n",
        )
        .unwrap();
        assert!(load_catalog_file(&bad).is_none(), "unsafe script path must be rejected");

        std::fs::write(&bad, "schema_version: 1\npresets: []\n").unwrap();
        assert!(load_catalog_file(&bad).is_some(), "a valid override loads");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_stage_catalog_leaves_local_override_untouched() {
        // The whole point of layer 1: a `presets update` refresh must not touch
        // catalog.local.yaml, so user presets survive every update.
        let dir = std::env::temp_dir().join(format!("restie-stage-override-{}", std::process::id()));
        let cached = dir.join("presets");
        std::fs::create_dir_all(&cached).unwrap();

        let local = cached.join("catalog.local.yaml");
        let local_body = "schema_version: 1\npresets:\n  - name: mine\n    display_name: Mine\n    openapi_url: https://mine.example/openapi.json\n    description: hand-written\n";
        std::fs::write(&local, local_body).unwrap();

        stage_catalog(&cached, "schema_version: 1\npresets: []\n", &[]).unwrap();

        assert_eq!(
            std::fs::read_to_string(&local).unwrap(),
            local_body,
            "presets update must never clobber the local override"
        );
        // ...while the fetched catalog itself was still refreshed.
        assert!(cached.join("catalog.yaml").is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Base catalog used by the merge tests: two plain presets.
    const CATALOG_WITH_TWO: &str = r#"
schema_version: 1
presets:
  - name: github
    display_name: GitHub
    openapi_url: https://example.com/gh.json
    description: base github
  - name: stripe
    display_name: Stripe
    openapi_url: https://example.com/stripe.json
    description: base stripe
"#;

    /// Local-override catalog: one replacement (`github`) plus one addition.
    const OVERRIDE_CATALOG: &str = r#"
schema_version: 1
presets:
  - name: github
    display_name: GitHub (patched)
    openapi_url: https://example.com/gh-v2.json
    description: overridden locally
  - name: internal
    display_name: Internal API
    openapi_url: https://internal.example/openapi.json
    description: added locally
    auth:
      type: bearer_token
      token_env: INTERNAL_API_TOKEN
"#;

    /// Minimal single-threaded HTTP server that serves a fixed file map, used to
    /// test the network path of `presets update` without any external service.
    fn serve_fixture_repo(listener: std::net::TcpListener, files: Vec<(&'static str, &'static str)>) {
        use std::io::{Read, Write};
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            if s.read(&mut buf).is_err() {
                continue;
            }
            let request = String::from_utf8_lossy(&buf);
            let path = request
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/");
            let (status, body) = match files.iter().find(|(p, _)| p == &path) {
                Some((_, content)) => ("200 OK", content.to_string()),
                None => ("404 Not Found", String::new()),
            };
            let response = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                body.len(),
                body
            );
            let _ = s.write_all(response.as_bytes());
        }
    }

    /// Fixture of a presets-repository catalog carrying the complex-auth,
    /// raw-only platforms that must NOT be in the bundled snapshot, plus a
    /// `scripts:` manifest naming the repo-managed signing scripts.
    const REPO_CATALOG: &str = r#"
schema_version: 1
scripts:
  - auth/aliyun-sign.sh
  - auth/tencent-sign.sh
presets:
  - name: aliyun
    display_name: Alibaba Cloud
    openapi_url: https://api.aliyun.com/meta/v1/products.json
    base_url: https://ecs.aliyuncs.com
    description: fixture
    auth:
      type: script
      command: auth/aliyun-sign.sh
      env:
        ALIYUN_ACCESS_KEY_ID: ${ALIBABA_CLOUD_ACCESS_KEY_ID}
        ALIYUN_ACCESS_KEY_SECRET: ${ALIBABA_CLOUD_ACCESS_KEY_SECRET}
  - name: tencent
    display_name: Tencent Cloud
    openapi_url: https://cvm.tencentcloudapi.com/
    base_url: https://cvm.tencentcloudapi.com
    description: fixture
    auth:
      type: script
      command: auth/tencent-sign.sh
      env:
        TENCENTCLOUD_SECRET_ID: ${TENCENTCLOUD_SECRET_ID}
        TENCENTCLOUD_SECRET_KEY: ${TENCENTCLOUD_SECRET_KEY}
        TENCENTCLOUD_SERVICE: cvm
        TENCENTCLOUD_REGION: ap-beijing
"#;

    fn repo_preset(name: &str) -> Preset {
        parse_catalog(REPO_CATALOG)
            .unwrap()
            .presets
            .into_iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("repo preset '{}' missing", name))
    }
}
