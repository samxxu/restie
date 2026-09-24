# AGENTS.md

Guidance for AI coding assistants working on **RESTie**, a transparent RESTful
API client CLI written in Rust. It turns any OpenAPI spec into a typed
command-line interface: `restie <module> <command> [--params]`.

Read this before editing. It captures the invariants the project is careful about.

## Repository layout

```
Cargo.toml            package metadata + reqwest/serde/tokio/clap deps
src/
  main.rs             CLI entrypoint, arg parsing, command dispatch
  cli.rs              command engine: param distribution, path replacement
  config.rs           YAML site schema (module/command hierarchy), load/find/help
  generator.rs        OpenAPI spec → YAML site config converter
  http_client.rs      HTTP core call() + GET/POST/PUT/PATCH/DELETE wrappers
  auth.rs             auth header injection (bearer/api_key/custom_header/script)
  presets.rs          preset catalog mechanism (fetch/validate/cache/resolve)
tests/
  http_client_test.rs integration tests (mockito)
```

The crate is both a library (`src/lib.rs` exports all six modules) and a
binary (`src/main.rs`).

## Build, test, lint

```bash
cargo build              # verify it compiles
cargo test               # run tests (integration tests use mockito, no network)
cargo clippy             # keep at zero warnings
cargo fmt                # follow rustfmt (edition 2021)
```

The project is developed spec-first and test-driven: write/update tests alongside
behavior changes and run the full suite before finishing.

## Architecture rules

- **Strict layering.** Keep the boundaries in `lib.rs`: `cli` (parsing) →
  `config` (schema) → `generator` (spec→config) → `http_client` (transport) →
  `auth` (headers). Preset fetching/resolution stays in `presets`.
- **Minimal, readable code.** Prefer small, obvious implementations over
  clever abstraction. Each module should be understandable in a short read.
- **`cfg/unused` hygiene.** Do not add dead code, unused params, or defensive
  branches for cases that cannot happen.

## Invariants and conventions

- **User-Agent** on every request (including spec downloads) is
  `concat!("restie/", env!("CARGO_PKG_VERSION"))` from `http_client.rs`
  (`USER_AGENT`). Do not hardcode the version string.
- **Config directory** is `~/.config/restie/` via `SiteConfig::config_dir()`.
  If `HOME` is unset it falls back to `/tmp/restie` with a warning to stderr.
- **CLI shape** is three-level: `restie <module> <command> [--params]`. Modules
  are derived from OpenAPI tags, plus built-ins: `generate`, `presets`
  (`presets update`), raw `GET`/`POST`/`PUT`/`PATCH`/`DELETE`, `list`,
  `sites`, `site set|clear`.
- **Command names** use kebab-case derived from the OpenAPI operation summary
  (e.g. `list-my-repos`), not raw `operationId`s.
- **HTTP methods reserved as top-level raw commands** and are capitalized
  (`GET`, `POST`, …).
- **`--help` must work for every command without making a request.**
- **Auth types**: `bearer_token`, `api_key`, `custom_header`, `script`, `none`
  (see `auth.rs`). Complex signing uses external scripts — in
  `~/.config/restie/auth/` — executed directly (no shell) to prevent injection.
- **Output contract:** stdout is always pure JSON (for pipelines); every status,
  warning, and error message goes to stderr. Empty / HTTP 204 responses are
  emitted as `null`, never as empty strings.
- **Validate required path parameters before sending**; missing or empty values
  raise an explicit error rather than letting the request fail opaquely.
- **CLI parameters keep their original API names** (e.g. `--per_page`, decoded
  from the spec), even if that clashes with shell conventions.
- **Header parameter names are normalized to Title-Case** (`x-api-key` →
  `X-Api-Key`) to avoid duplicate headers.
- **Config files are YAML** with a `site` top level containing `auth` and
  `headers`. `.yaml` takes priority over `.yml`; generated configs are named
  from the spec's `info.title` (slugified).
- **Every request goes through `http_client::call()`** so UA, proxies
  (incl. SOCKS via the `socks` feature) and auth injection stay consistent.

## Presets subsystem (`src/presets.rs`)

Presets are **data fetched from a separate repository**, not built into the
binary. The core is only the mechanism: fetch → validate → cache → resolve.

- Remote repository: `https://github.com/samxxu/restie-presets` (raw,
  `https://raw.githubusercontent.com/samxxu/restie-presets/main/...`).
- Cache path: `~/.config/restie/presets/` (`catalog.yaml` + any `auth/*.sh`).
- **Three-tier resolution:** local override `catalog.local.yaml` (always wins)
  > remote cache > none. `presets update` must never touch `catalog.local.yaml`.
- **Atomic writes:** download and validate everything first, then write via a
  temp file + atomic rename, so a partial download cannot corrupt the cache.
- **Catalog validation** (`parse_catalog`): `schema_version` must be supported;
  preset `name`s unique; required fields non-empty; unknown fields rejected;
  script paths relative and free of `..` / absolute segments.
- First run auto-bootstraps (`bootstrap_if_needed`): if nothing is cached it
  fetches once, printing intent to stderr before hitting the network.

## Environment / config overview

- `~/.config/restie/*.yaml` — per-site configs (`site:`, `auth:`, `headers:`).
- `~/.config/restie/presets/catalog.yaml` — cached remote catalog.
- `~/.config/restie/presets/catalog.local.yaml` — user-local overrides.
- `~/.config/restie/auth/` — auth scripts (for `type: script`).
- `RESTIE_PRESETS_REPO` env var — overrides the default raw repo base for
  `restie presets update`.

## When editing

1. Update `presets.rs` and its callers together; keep the three-tier resolution
   and atomic-write guarantees intact.
2. Run `cargo test` and `cargo clippy` (zero warnings) before finishing.
3. Keep user-facing prose in stderr and never change the JSON contract of stdout.