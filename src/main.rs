// =============================================================================
// RESTie - Transparent RESTful API Client CLI
// Three-level structure: restie <module> <command> [--params]
// =============================================================================

#![allow(non_snake_case)]

use clap::{Arg, ArgAction, Command};
use restie::{
    cli as cli_engine,
    config::{SiteConfig, SiteInfo},
    generator,
    http_client::HttpClient,
    presets,
};
use std::collections::HashMap;

fn build_cli() -> Command {
    Command::new("restie")
        .version(env!("CARGO_PKG_VERSION"))
        .about("RESTie - A transparent RESTful API client")
        .args_override_self(true)
        .allow_external_subcommands(true)
        .arg(
            Arg::new("config")
                .long("config")
                .global(true)
                .help("Config file path (overrides auto-discovery)"),
        )
        .arg(
            Arg::new("site")
                .long("site")
                .global(true)
                .help("Site name (e.g. github, vultr) — loads ~/.config/restie/<site>.yaml"),
        )
        .arg(
            Arg::new("raw")
                .long("raw")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Output raw JSON without pretty-printing"),
        )
        .subcommand(
            Command::new("generate")
                .about("Generate config from a preset name or OpenAPI spec URL/file")
                .arg(Arg::new("source")
                    .index(1)
                    .help("Preset name (e.g. github, gitlab, stripe) or URL/path to OpenAPI spec"))
                .arg(Arg::new("from").long("from").help("URL or local path to OpenAPI spec"))
                .arg(Arg::new("filter").long("filter").help("Filter endpoints by path substring"))
                .arg(Arg::new("output").long("output").help("Output file path (default: ~/.config/restie/<name>.yaml)"))
                .arg(Arg::new("name").long("name").help("Override site name for auto-naming")),
        )
        .subcommand(
            Command::new("presets")
                .about("List platform presets, or update them from the presets repository")
                .subcommand(Command::new("update")
                    .about("Fetch the latest preset catalog + signing scripts from the presets repository")
                    .arg(Arg::new("repo").long("repo")
                        .help("Presets repository raw base URL (default: RESTIE_PRESETS_REPO env var, then the built-in default)"))),
        )
        .subcommand(
            Command::new("GET")
                .about("Make a raw GET request")
                .arg(Arg::new("path").required(true).help("API path")),
        )
        .subcommand(
            Command::new("POST")
                .about("Make a raw POST request")
                .arg(Arg::new("path").required(true).help("API path"))
                .arg(Arg::new("body").help("JSON body")),
        )
        .subcommand(
            Command::new("PUT")
                .about("Make a raw PUT request")
                .arg(Arg::new("path").required(true).help("API path"))
                .arg(Arg::new("body").help("JSON body")),
        )
        .subcommand(
            Command::new("PATCH")
                .about("Make a raw PATCH request")
                .arg(Arg::new("path").required(true).help("API path"))
                .arg(Arg::new("body").help("JSON body")),
        )
        .subcommand(
            Command::new("DELETE")
                .about("Make a raw DELETE request")
                .arg(Arg::new("path").required(true).help("API path")),
        )
        .subcommand(
            Command::new("list")
                .about("List all modules and commands from config"),
        )
        .subcommand(
            Command::new("sites")
                .about("List available site configs in ~/.config/restie/"),
        )
        .subcommand(
            Command::new("site")
                .about("Get or set the active site (used when --site is omitted)")
                .subcommand(Command::new("set").about("Set the active site").arg(
                    Arg::new("name").index(1).required(true).help("Site name"),
                ))
                .subcommand(Command::new("clear").about("Clear the active site")),
        )
}

// Print to stdout, exiting quietly when the pipe closes (e.g. `restie list | head`).
// Shadows std::println! so every stdout print is protected against SIGPIPE panics.
macro_rules! println {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        if writeln!(stdout, $($arg)*).is_err() {
            std::process::exit(0);
        }
    }};
}

#[tokio::main]
async fn main() {
    let matches = build_cli().get_matches();
    let config_path = matches.get_one::<String>("config").map(|s| s.as_str());
    let site = matches.get_one::<String>("site").map(|s| s.as_str());
    let raw = matches.get_flag("raw");

    match matches.subcommand() {
        Some(("generate", sub)) => {
            let source = sub.get_one::<String>("source").map(|s| s.as_str());
            let from_flag = sub.get_one::<String>("from").map(|s| s.as_str());
            let filter = sub.get_one::<String>("filter").map(|s| s.as_str());
            let output = sub.get_one::<String>("output").cloned();
            let name_override = sub.get_one::<String>("name").cloned();

            // Determine the source: positional arg takes form, --from flag second
            let from = match (source, from_flag) {
                (Some(s), _) => s,
                (_, Some(f)) => f,
                (None, None) => {
                    eprintln!("Error: specify a preset name or use --from <URL/path>");
                    eprintln!("Run 'restie presets' to see available presets.");
                    std::process::exit(1);
                }
            };

            // Only a preset name needs the catalog; a spec URL or a local file
            // is self-contained, so skip the fetch entirely in that case.
            if !looks_like_spec_source(from) {
                ensure_presets().await;
            }

            cmd_generate(from, filter, output, name_override).await;
        }
        Some(("presets", sub)) => {
            if let Some(update) = sub.subcommand_matches("update") {
                let repo = update.get_one::<String>("repo").map(|s| s.as_str());
                cmd_presets_update(repo).await;
            } else {
                ensure_presets().await;
                cmd_presets();
            }
        }
        Some(("GET", sub)) => {
            let path = sub.get_one::<String>("path").unwrap();
            cmd_raw("GET", path, None, config_path, site, raw).await;
        }
        Some(("POST", sub)) => {
            let path = sub.get_one::<String>("path").unwrap();
            let body = sub.get_one::<String>("body");
            cmd_raw(
                "POST",
                path,
                body.map(|s| s.as_str()),
                config_path,
                site,
                raw,
            )
            .await;
        }
        Some(("PUT", sub)) => {
            let path = sub.get_one::<String>("path").unwrap();
            let body = sub.get_one::<String>("body");
            cmd_raw(
                "PUT",
                path,
                body.map(|s| s.as_str()),
                config_path,
                site,
                raw,
            )
            .await;
        }
        Some(("PATCH", sub)) => {
            let path = sub.get_one::<String>("path").unwrap();
            let body = sub.get_one::<String>("body");
            cmd_raw(
                "PATCH",
                path,
                body.map(|s| s.as_str()),
                config_path,
                site,
                raw,
            )
            .await;
        }
        Some(("DELETE", sub)) => {
            let path = sub.get_one::<String>("path").unwrap();
            cmd_raw("DELETE", path, None, config_path, site, raw).await;
        }
        Some(("list", _)) => {
            ensure_presets().await;
            cmd_list(config_path, site);
        }
        Some(("sites", _)) => {
            ensure_presets().await;
            cmd_sites();
        }
        Some(("site", sub)) => {
            cmd_site(sub);
        }
        Some((name, sub)) => {
            let args: Vec<String> = sub
                .get_many::<std::ffi::OsString>("")
                .map(|vals| {
                    vals.filter_map(|v| v.to_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            cmd_module(name, &args, config_path, site, raw).await;
        }
        None => {
            ensure_presets().await;
            cmd_help(config_path, site);
        }
    }
}

/// Whether a `generate` source is a spec URL or an existing local file, i.e.
/// something that stands on its own and needs no preset catalog.
fn looks_like_spec_source(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://") || std::path::Path::new(s).exists()
}

/// First-run convenience: if no preset catalog has ever been fetched, fetch one
/// now so the user never lands on an empty preset list without knowing why.
///
/// Runs at most once per machine: later invocations find the cache and do no
/// network I/O. A failure is not fatal, since raw requests and `generate --from`
/// work without any catalog; the command that needed presets reports the gap.
///
/// Progress goes to stderr, never stdout: stdout carries API responses, so
/// `restie repos get ... | jq` must not have a fetch notice spliced into it.
async fn ensure_presets() {
    if presets::cached_catalog_exists() {
        return;
    }

    // Say what is about to happen *before* waiting on the network. Otherwise a
    // slow or blocked link looks like RESTie hanging for no reason, which is
    // exactly what a first run gets blamed for.
    if let Ok(base) = presets::default_repo_base() {
        eprintln!("No preset catalog cached yet; fetching it from {}", base);
    }

    match presets::bootstrap_if_needed().await {
        presets::Bootstrap::AlreadyCached => {}
        presets::Bootstrap::Fetched(summary) => {
            eprintln!(
                "Cached {} presets and {} signing script(s) in {}",
                summary.preset_count,
                summary.script_count,
                presets::cache_dir().display()
            );
        }
        presets::Bootstrap::Failed(e) => {
            eprintln!("Warning: could not fetch the preset catalog: {}", e);
            eprintln!("Presets stay empty for now; retry with 'restie presets update'.");
        }
    }
}

async fn cmd_generate(
    from: &str,
    filter: Option<&str>,
    output: Option<String>,
    name_override: Option<String>,
) {
    // Check if `from` is a preset name
    let preset = presets::find_preset(from);

    // Raw-only path: a script-auth preset has no usable OpenAPI spec (e.g.
    // Aliyun, Tencent Cloud), so we never fetch its openapi_url. Instead we
    // write a site + auth config (no commands) and let the signing script
    // build the auth headers at raw-request time.
    if let Some(p) = &preset {
        if let Some(a) = &p.default_auth {
            if a.auth_type == "script" {
                generate_raw_config(p, name_override, output);
                return;
            }
        }
    }

    let (actual_url, site_name_hint, preset_auth) = match preset {
        Some(p) => (p.openapi_url, Some(p.name.clone()), p.default_auth),
        None => {
            // Not a preset: must be a URL or an existing local file
            let is_url = from.starts_with("http://") || from.starts_with("https://");
            let is_file = std::path::Path::new(from).exists();
            if !is_url && !is_file {
                eprintln!("Error: unknown preset '{}'.", from);
                if presets::effective_catalog().presets.is_empty() {
                    eprintln!("No preset catalog is available yet, so no presets exist to match.");
                    eprintln!("Fetch one with 'restie presets update'.");
                } else {
                    eprintln!("Run 'restie presets' to see available presets.");
                }
                std::process::exit(1);
            }
            (from.to_string(), None, None)
        }
    };

    let content = if actual_url.starts_with("http://") || actual_url.starts_with("https://") {
        // Always send our User-Agent on spec downloads too — some hosts (e.g.
        // GitHub raw) reject or throttle requests with a foreign/absent UA.
        //
        // This client needs its own timeouts: specs are the largest thing RESTie
        // downloads (GitHub's is ~10 MB), and without a bound a stalled connection
        // hangs the command for minutes before failing with no explanation.
        let client = match reqwest::Client::builder()
            .user_agent(restie::http_client::USER_AGENT)
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(300))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error building HTTP client: {}", e);
                std::process::exit(1);
            }
        };
        match client.get(actual_url.as_str()).send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    eprintln!(
                        "Error fetching OpenAPI spec: HTTP {} from {}",
                        resp.status(),
                        actual_url
                    );
                    std::process::exit(1);
                }
                match resp.text().await {
                    Ok(body) => body,
                    Err(e) => {
                        eprintln!(
                            "Error reading OpenAPI spec from {}: {}\n\
                             The server may have closed the connection early.",
                            actual_url, e
                        );
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error fetching OpenAPI spec: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        match std::fs::read_to_string(&actual_url) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error reading file: {}", e);
                std::process::exit(1);
            }
        }
    };

    // An empty body parses as null in YAML and would otherwise surface much later
    // as "no endpoints", which hides the real problem.
    if content.trim().is_empty() {
        eprintln!(
            "Error: OpenAPI spec at '{}' is empty — nothing to generate from.",
            actual_url
        );
        std::process::exit(1);
    }

    let mut config = match generator::generate_config(&content, filter) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error parsing OpenAPI spec: {}", e);
            std::process::exit(1);
        }
    };

    // Inject preset auth config if available
    if let Some(auth) = preset_auth {
        if let Some(ref mut site_info) = config.site {
            site_info.auth = Some(auth);
        }
    }

    // Auto-name: preset name > --name > spec title
    let site_name = name_override.or(site_name_hint).unwrap_or_else(|| {
        let title = config
            .site
            .as_ref()
            .and_then(|s| s.name.as_deref())
            .unwrap_or("site");
        generator::site_name_to_filename(title)
    });

    // Determine output path
    let out_path = output.unwrap_or_else(|| {
        let dir = SiteConfig::config_dir();
        dir.join(format!("{}.yaml", site_name))
            .to_string_lossy()
            .to_string()
    });

    // Refuse to write a useless empty config (e.g. spec URL returned an error page)
    let total: usize = config.modules.values().map(|m| m.commands.len()).sum();
    if total == 0 {
        eprintln!("Error: OpenAPI spec produced 0 endpoints — nothing to generate.");
        eprintln!("Check that the URL points to a valid OpenAPI spec and the --filter is correct.");
        std::process::exit(1);
    }

    // Refuse to write into an existing directory (user probably passed a
    // directory to --output instead of a file path).
    if std::path::Path::new(&out_path).is_dir() {
        eprintln!(
            "Error: output path '{}' is a directory. Use --output <file>.yaml",
            out_path
        );
        std::process::exit(1);
    }

    let yaml = match generator::to_yaml(&config) {
        Ok(y) => y,
        Err(e) => {
            eprintln!("Error generating YAML: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = std::fs::write(&out_path, &yaml) {
        eprintln!("Error writing {}: {}", out_path, e);
        std::process::exit(1);
    }

    println!(
        "Generated {} (site: {}, {} modules, {} endpoints)",
        out_path,
        site_name,
        config.modules.len(),
        total
    );
}

/// Write a raw-only config (just `site:` with base_url + auth, no commands)
/// for a script-auth preset that has no usable OpenAPI spec (Aliyun, Tencent
/// Cloud). Such platforms are called in raw mode (`restie --site <name> <METHOD>
/// /path`), where the installed signing script produces the auth headers at
/// request time from the RESTIE_* context vars.
fn generate_raw_config(
    preset: &presets::Preset,
    name_override: Option<String>,
    output: Option<String>,
) {
    let site_name = name_override.unwrap_or_else(|| preset.name.clone());
    let base_url = preset
        .base_url
        .clone()
        .unwrap_or_else(|| "https://example.com".to_string());

    let config = SiteConfig {
        site: Some(SiteInfo {
            name: Some(site_name.clone()),
            base_url: Some(base_url),
            auth: preset.default_auth.clone(),
            headers: None,
        }),
        modules: std::collections::BTreeMap::new(),
    };

    let yaml = match generator::to_yaml(&config) {
        Ok(y) => y,
        Err(e) => {
            eprintln!("Error generating YAML: {}", e);
            std::process::exit(1);
        }
    };

    let out_path = output.unwrap_or_else(|| {
        let dir = SiteConfig::config_dir();
        dir.join(format!("{}.yaml", site_name))
            .to_string_lossy()
            .to_string()
    });

    if std::path::Path::new(&out_path).is_dir() {
        eprintln!(
            "Error: output path '{}' is a directory. Use --output <file>.yaml",
            out_path
        );
        std::process::exit(1);
    }

    if let Err(e) = std::fs::write(&out_path, &yaml) {
        eprintln!("Error writing {}: {}", out_path, e);
        std::process::exit(1);
    }

    // The signing script the config points at was staged by `restie presets
    // update` (into ~/.config/restie/presets/) and resolves from there, so there
    // is nothing to install here.
    println!(
        "Generated {} (site: {}, raw-only — no commands)\n\
         Call it in raw mode: restie --site {} <METHOD> /path?a=1&b=2\n\
         Edit site.base_url in ~/.config/restie/{}.yaml, or the signing script \
         under ~/.config/restie/presets/auth/, if your product's host differs.",
        out_path, site_name, site_name, site_name
    );
}

/// Display string for the "Env Variable" column in the presets table.
/// For token-based auth, shows the single token env var.
/// For script auth, picks the most "important" env var (key/secret/token),
/// falling back to the first one alphabetically.
fn preset_env_display(auth: Option<&restie::config::AuthConfig>) -> String {
    let auth = match auth {
        Some(a) => a,
        None => return "-".to_string(),
    };
    if let Some(tok) = &auth.token_env {
        return tok.clone();
    }
    if let Some(env_map) = &auth.env {
        // Prefer variables that look like credentials.
        let priority_keywords = ["SECRET_ID", "ACCESS_KEY_ID", "API_KEY", "TOKEN", "SECRET"];
        for keyword in &priority_keywords {
            if let Some(key) = env_map.keys().find(|k| k.contains(keyword)) {
                return key.clone();
            }
        }
        if let Some((first_key, _)) = env_map.iter().next() {
            return first_key.clone();
        }
    }
    "-".to_string()
}

fn cmd_presets() {
    let all = presets::all_presets();

    // Nothing fetched (and no local override): explain where presets come from
    // rather than printing an empty table.
    if all.is_empty() {
        println!("No presets available yet.\n");
        println!("Preset data lives in the presets repository, not in the RESTie binary,");
        println!("so the list can be updated without a new RESTie release.");
        println!("\nFetch it:");
        println!("  restie presets update");
        println!("\nUse a different repository:");
        println!("  restie presets update --repo <raw-base-url>");
        println!("  export RESTIE_PRESETS_REPO=<raw-base-url>   # default for future runs");
        println!("\nAdd your own without a repo:");
        println!("  {}", presets::local_override_path().display());
        println!("\nOr skip presets entirely and generate from any OpenAPI spec:");
        println!("  restie generate --from <url-or-path>");
        return;
    }

    // Compute column widths dynamically so new presets never break alignment.
    let mut w_name = "Name".len();
    let mut w_auth = "Auth Type".len();
    let mut w_env = "Env Variable".len();
    for p in &all {
        w_name = w_name.max(p.name.len());
        let at = p
            .default_auth
            .as_ref()
            .map(|a| a.auth_type.len())
            .unwrap_or(4);
        w_auth = w_auth.max(at);
        let ev = preset_env_display(p.default_auth.as_ref()).len();
        w_env = w_env.max(ev);
    }

    let header = format!(
        "  {:<w1$} {:<w2$} {:<w3$} Description",
        "Name",
        "Auth Type",
        "Env Variable",
        w1 = w_name + 2,
        w2 = w_auth + 2,
        w3 = w_env + 2
    );
    let sep = format!(
        "  {:<w1$} {:<w2$} {:<w3$} -----------",
        "----",
        "---------",
        "------------",
        w1 = w_name + 2,
        w2 = w_auth + 2,
        w3 = w_env + 2
    );

    println!("Available platform presets:\n");
    println!("{}", header);
    println!("{}", sep);
    for p in all {
        let auth_type = p
            .default_auth
            .as_ref()
            .map(|a| a.auth_type.as_str())
            .unwrap_or("none");
        let env_var = preset_env_display(p.default_auth.as_ref());
        println!(
            "  {:<w1$} {:<w2$} {:<w3$} {}",
            p.name,
            auth_type,
            env_var,
            p.description,
            w1 = w_name + 2,
            w2 = w_auth + 2,
            w3 = w_env + 2
        );
    }
    println!("\nUsage:");
    println!("  restie generate <preset>           Generate config for a preset");
    println!("  restie generate --from <url>      Generate from custom OpenAPI spec");
    println!("  restie generate --from <url> --filter <path>   Filter by path substring");
    println!(
        "\nCatalog: schema v{} — {} ({} presets)",
        presets::SCHEMA_VERSION,
        presets::effective_description(),
        presets::effective_catalog().presets.len()
    );
    println!("Refresh/expand presets anytime: restie presets update");
    println!(
        "Add or override presets locally: {} (never overwritten by update)",
        presets::local_override_path().display()
    );
}

/// Fetch the latest preset catalog + signing scripts from the presets
/// repository into ~/.config/restie/presets/ and report what changed.
async fn cmd_presets_update(repo: Option<&str>) {
    match presets::update_from_repo(repo).await {
        Ok(summary) => {
            println!("Updated presets from {}", summary.catalog_url);
            println!(
                "  {} presets, {} signing script(s) staged in {}",
                summary.preset_count,
                summary.script_count,
                presets::cache_dir().display()
            );
            println!("Presets now come from the remote catalog. Run 'restie presets' to browse.");
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Get or set the active site.
fn cmd_site(sub: &clap::ArgMatches) {
    match sub.subcommand() {
        Some(("set", set_sub)) => {
            let name = set_sub.get_one::<String>("name").unwrap();
            match SiteConfig::set_current_site(name) {
                Ok(()) => println!("Active site set to '{}'. Omit --site to use it.", name),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Some(("clear", _)) => {
            SiteConfig::clear_current_site();
            println!("Active site cleared. Fall back to auto-selection.");
        }
        _ => match SiteConfig::active_site() {
            Some(name) => println!("Active site: {}", name),
            None => {
                println!("No active site set. Pass --site <name> or use 'restie site set <name>'.")
            }
        },
    }
}

async fn cmd_raw(
    method: &str,
    path: &str,
    body: Option<&str>,
    config_path: Option<&str>,
    site: Option<&str>,
    raw: bool,
) {
    let config = SiteConfig::find_and_load(config_path, site);
    let client = match config.as_ref() {
        Some(cfg) => HttpClient::new(cfg),
        None => HttpClient::with_base("https://api.github.com"),
    };

    let body_val =
        body.map(|b| serde_json::from_str(b).unwrap_or(serde_json::Value::String(b.to_string())));

    let result = match method {
        "GET" => client.GET(path, None).await,
        "POST" => client.POST(path, body_val, None).await,
        "PUT" => client.PUT(path, body_val, None).await,
        "PATCH" => client.PATCH(path, body_val, None).await,
        "DELETE" => client.DELETE(path, None).await,
        _ => {
            eprintln!("Unknown method: {}", method);
            std::process::exit(1);
        }
    };

    output_result(result, raw);
}

fn cmd_help(config_path: Option<&str>, site: Option<&str>) {
    let config = SiteConfig::find_and_load(config_path, site);

    match config {
        Some(ref config) => {
            println!("{}", config.list_modules());
            if config.modules.is_empty() {
                println!("Note: this config has no commands. Regenerate it (e.g. 'restie generate <preset>') or delete the file.\n");
            }
            if site.is_none() && config_path.is_none() {
                print_site_hint();
            }
            println!("\nUse 'restie <module> --help' to see commands in a module.");
            println!("Use 'restie list' to see all commands.");
            println!("Use 'restie sites' to see available site configs.");
        }
        None => {
            // No config yet — show presets as onboarding
            println!("RESTie - Transparent RESTful API Client\n");
            println!("No config file found yet. Get started in one command:\n");
            cmd_presets();
        }
    }
}

fn cmd_list(config_path: Option<&str>, site: Option<&str>) {
    let config = match SiteConfig::find_and_load(config_path, site) {
        Some(c) => c,
        None => {
            println!("No config file found yet. Get started by generating a config:\n");
            cmd_presets();
            return;
        }
    };

    println!("{}", config.list_all_commands());
    if config.modules.is_empty() {
        println!("Note: this config has no commands. Regenerate it (e.g. 'restie generate <preset>') or delete the file.\n");
    }
    if site.is_none() && config_path.is_none() {
        print_site_hint();
    }
}

/// If multiple site configs exist, tell the user which one was auto-selected.
fn print_site_hint() {
    let sites = SiteConfig::available_sites();
    if sites.len() > 1 {
        match SiteConfig::active_site() {
            Some(current) => println!(
                "\nMultiple site configs found ({}). Active: '{}' — use 'restie site set <name>' or --site <name> to switch.",
                sites.join(", "),
                current
            ),
            None => println!(
                "\nMultiple site configs found ({}). No active site set — pass --site <name>, or set one with 'restie site set <name>'.",
                sites.join(", ")
            ),
        }
    }
}

fn cmd_sites() {
    let dir = SiteConfig::config_dir();
    let entries: Vec<_> = if dir.is_dir() {
        std::fs::read_dir(&dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let path = e.path();
                let ext = path.extension().and_then(|x| x.to_str());
                ext == Some("yaml") || ext == Some("yml")
            })
            .collect()
    } else {
        Vec::new()
    };

    if entries.is_empty() {
        println!("No site configs found in {}", dir.display());
        println!("Get started by generating one:\n");
        cmd_presets();
        return;
    }

    println!("Available sites in {}:\n", dir.display());
    let mut entries = entries;
    entries.sort_by(|a, b| {
        let na = a.file_name().to_string_lossy().to_string().to_lowercase();
        let nb = b.file_name().to_string_lossy().to_string().to_lowercase();
        na.cmp(&nb)
    });

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let name = name
            .strip_suffix(".yaml")
            .or_else(|| name.strip_suffix(".yml"))
            .unwrap_or(&name);
        println!("  {}", name);
    }
}

/// Handle <module> [command] [--params ...]
async fn cmd_module(
    module: &str,
    args: &[String],
    config_path: Option<&str>,
    site: Option<&str>,
    raw: bool,
) {
    // --raw / --site / --config are control flags, not API parameters.
    // Strip them from the args so they never leak into the request payload,
    // and let site/config written *after* the module override the pre-module
    // values (users naturally write `restie repos get --site github`).
    let (filtered, raw_flag, site_flag, config_flag) = cli_engine::strip_control_flags(args);
    let raw = raw || raw_flag;
    let site = site_flag.as_deref().or(site);
    let config_path = config_flag.as_deref().or(config_path);

    let (command, remaining_args) = split_command_and_args(&filtered);
    let params = parse_extra_args(&remaining_args);

    // No command → show module help (or onboarding if no config)
    if command.is_empty() {
        let config = SiteConfig::find_and_load(config_path, site);
        match config {
            Some(ref cfg) => {
                println!("{}", cfg.help_for_module(module));
            }
            None => {
                // Maybe the user typed a built-in subcommand by mistake (e.g. "preset" vs "presets")
                if let Some(suggestion) = suggest_command(module) {
                    eprintln!(
                        "Unknown command: '{}'. Did you mean '{}'?",
                        module, suggestion
                    );
                    eprintln!("Run 'restie presets' to see available platforms.");
                } else {
                    println!("No config file found yet. Get started by generating a config:\n");
                    cmd_presets();
                }
            }
        }
        return;
    }

    // --help → show command help
    if params.contains_key("help") {
        let config = SiteConfig::find_and_load(config_path, site);
        match config {
            Some(ref cfg) => {
                println!("{}", cfg.help_for_command(module, &command));
            }
            None => {
                println!("No config file found yet. Get started by generating a config:\n");
                cmd_presets();
            }
        }
        return;
    }

    // Execute the API call
    let config = SiteConfig::find_and_load(config_path, site);
    let client = match config.as_ref() {
        Some(cfg) => HttpClient::new(cfg),
        None => {
            eprintln!("No config file found yet. Get started by generating a config:\n");
            cmd_presets();
            std::process::exit(1);
        }
    };

    let parts = if let Some(ref cfg) = config {
        if let Some(ep) = cfg.get_endpoint(module, &command) {
            match cli_engine::distribute_params_with_config(ep, &params) {
                Ok(parts) => parts,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    eprintln!(
                        "Run 'restie {} {} --help' to see required parameters.",
                        module, command
                    );
                    std::process::exit(1);
                }
            }
        } else {
            eprintln!("Command '{} {}' not found in config.", module, command);
            eprintln!("Available commands in module '{}':", module);
            eprintln!("{}", cfg.help_for_module(module));
            std::process::exit(1);
        }
    } else {
        unreachable!()
    };

    let result = cli_engine::execute(&client, parts).await;
    output_result(result, raw);
}

fn split_command_and_args(args: &[String]) -> (String, Vec<String>) {
    for (i, arg) in args.iter().enumerate() {
        if !arg.starts_with("--") {
            let command = arg.clone();
            let remaining = args[i + 1..].to_vec();
            return (command, remaining);
        }
    }
    (String::new(), args.to_vec())
}

fn parse_extra_args(args: &[String]) -> HashMap<String, String> {
    let mut params = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(rest) = arg.strip_prefix("--") {
            if let Some((name, value)) = rest.split_once('=') {
                params.insert(name.to_string(), value.to_string());
                i += 1;
            } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                params.insert(rest.to_string(), args[i + 1].clone());
                i += 2;
            } else {
                params.insert(rest.to_string(), "true".to_string());
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    params
}

fn output_result(result: Result<serde_json::Value, Box<dyn std::error::Error>>, raw: bool) {
    match result {
        Ok(value) => {
            // Keep stdout pure JSON so pipelines (`jq`, etc.) work unchanged.
            if raw {
                println!("{}", value);
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or(value.to_string())
                );
            }
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

/// Suggest a correct built-in command for common typos.
fn suggest_command(input: &str) -> Option<&'static str> {
    let builtins = [
        "generate", "presets", "list", "sites", "GET", "POST", "PUT", "PATCH", "DELETE",
    ];
    let mut best: Option<(&str, usize)> = None;

    for name in builtins {
        let dist = levenshtein(input, name);
        if dist <= 2 && (best.is_none() || dist < best.unwrap().1) {
            best = Some((name, dist));
        }
    }

    best.map(|(name, _)| name)
}

/// Simple Levenshtein distance for fuzzy matching.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let (n, m) = (a_chars.len(), b_chars.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }

    let mut d = vec![vec![0; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }

    for i in 1..=n {
        for j in 1..=m {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            d[i][j] = *[d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + cost]
                .iter()
                .min()
                .unwrap();
        }
    }
    d[n][m]
}
