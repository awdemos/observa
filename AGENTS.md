# PROJECT KNOWLEDGE BASE

**Generated:** 2026-07-20
**Commit:** 5fc5547
**Branch:** main

## OVERVIEW

Observa is a real-time system observability dashboard written in Rust. It is a Cargo workspace with 10 crates: a single binary (`observa-cli`) composes a collector, ingestor, SQLite store, optional Redis cache, SSE bus, LLM client, and an Axum/Tera HTTP server that renders a dark, terminal-inspired web UI.

## STRUCTURE

```
.
├── crates/           # Rust workspace crates (see individual AGENTS.md)
│   ├── observa-cli      # Executable entry point
│   ├── observa-server   # HTTP dashboard, API, auth, chat, background tasks
│   ├── observa-collector# Metrics sampler + AI-server discovery
│   ├── observa-db       # SQLite persistence and migrations
│   ├── observa-llm      # OpenAI-compatible chat client
│   ├── observa-config   # Config, CLI, tracing, shutdown
│   ├── observa-ingestor # Log/journal ingestion
│   ├── observa-cache    # Redis wrapper with in-memory fallback
│   ├── observa-bus      # SSE broadcast bus
│   └── observa-shared   # Domain types and errors
├── templates/        # Tera HTML templates and HTMX partials
├── assets/           # CSS, JS, vendor Three.js
├── docs/             # Screenshots and documentation
├── observa.toml      # Default runtime configuration
├── Dockerfile        # Multi-stage release build
├── docker-compose*.yml
└── run-docker.sh     # Podman wrapper for local deployment
```

## WHERE TO LOOK

| Task | Location | Notes |
|---|---|---|
| Run the app | `/var/home/a/code/observa/crates/observa-cli/src/main.rs` | Only binary crate; composes all subsystems |
| HTTP routes | `/var/home/a/code/observa/crates/observa-server/src/routes/mod.rs` | Full dashboard router + middleware |
| REST API | `/var/home/a/code/observa/crates/observa-server/src/api.rs` | `/api/*` endpoints |
| Auth | `/var/home/a/code/observa/crates/observa-server/src/auth.rs` | Dashboard/chat token middleware |
| Chat | `/var/home/a/code/observa/crates/observa-server/src/chat.rs` | Ask/stream handlers, session logic |
| Metric collection | `/var/home/a/code/observa/crates/observa-collector/src/normalize.rs` | GPU fallback via `/sys/class/drm` and PCI |
| Persistence | `/var/home/a/code/observa/crates/observa-db/src/` | Per-domain modules + migrations |
| UI templates | `/var/home/a/code/observa/templates/` | Extend `base.html`; partials for HTMX |
| Theme CSS | `/var/home/a/code/observa/assets/css/observa.css` | Dark theme tokens, responsive rules |

## CODE MAP

| Symbol | Type | Location | Role |
|---|---|---|---|
| `main` | function | `/var/home/a/code/observa/crates/observa-cli/src/main.rs:18` | Application entry point |
| `serve_with_shutdown` | function | `/var/home/a/code/observa/crates/observa-server/src/lib.rs:29` | Binds Axum server |
| `router` | function | `/var/home/a/code/observa/crates/observa-server/src/routes/mod.rs:45` | Full dashboard router |
| `AppState` | struct | `/var/home/a/code/observa/crates/observa-server/src/state.rs` | Shared runtime state |
| `normalize` | function | `/var/home/a/code/observa/crates/observa-collector/src/normalize.rs` | Metric snapshot builder |
| `spawn_collector` | function | `/var/home/a/code/observa/crates/observa-collector/src/collector.rs` | Background metric sampler |
| `Db` | struct | `/var/home/a/code/observa/crates/observa-db/src/pool.rs` | SQLite connection pool |
| `MetricStore` / `ChatStore` | trait | `/var/home/a/code/observa/crates/observa-server/src/store.rs` | Storage seams |
| `LlmClient` | struct | `/var/home/a/code/observa/crates/observa-llm/src/client.rs` | OpenAI-compatible client |
| `Bus` | struct | `/var/home/a/code/observa/crates/observa-bus/src/lib.rs` | SSE broadcast bus |

## CONVENTIONS

- Rust edition 2021; workspace `resolver = "2"`.
- No custom `rustfmt.toml`, `clippy.toml`, or `.editorconfig`; default toolchain formatting applies.
- Single quotes, no semicolons in shell scripts (`run-docker.sh`).
- TypeScript strict mode is **not** relevant here; this is a Rust project. The old AGENTS.md was a stale template.
- Each crate has a `lib.rs` that re-exports its public surface; modules live in `src/<module>.rs` or `src/<module>/mod.rs`.

## ANTI-PATTERNS (THIS PROJECT)

- `unsafe` Rust blocks: none in source. The word `unsafe` only appears in the CSP string.
- 3 production `.expect()` calls remain (see `/var/home/a/code/observa/crates/observa-llm/src/client.rs`, `/var/home/a/code/observa/crates/observa-ingestor/src/reader.rs`, `/var/home/a/code/observa/crates/observa-server/src/insight.rs`).
- Blocking synchronous I/O (`std::fs`, `std::process::Command`) inside async paths in `/var/home/a/code/observa/crates/observa-collector/src/normalize.rs` and `/var/home/a/code/observa/crates/observa-server/src/tpe.rs`.
- CSP allows `'unsafe-inline'` and external resources (`unpkg.com`, Google Fonts) in `/var/home/a/code/observa/crates/observa-server/src/routes/mod.rs`.

## UNIQUE STYLES

- Only one binary crate (`observa-cli`); the "server" crate is a library consumed by it.
- Static web assets and Tera templates live at the workspace root, not inside the server crate.
- Runtime config file `observa.toml` is at the workspace root and overridable via `OBSERVA_*` env vars.
- Chat sessions are persisted in SQLite and identified by an owner token in a cookie.

## COMMANDS

```bash
# Build
cargo build --workspace

# Run locally (open http://127.0.0.1:3000)
cargo run -p observa-cli

# Tests
cargo test --workspace

# Lint (warnings-as-errors policy from README)
cargo clippy --workspace --all-targets -- -D warnings

# Local Podman container
bash run-docker.sh
```

## NOTES

- `run-docker.sh` runs Podman, not Docker, and includes Podman-specific cgroup flags for this nested container environment.
- The README still describes an older Chainguard/cargo-chef Dockerfile that no longer matches the actual Dockerfile.
