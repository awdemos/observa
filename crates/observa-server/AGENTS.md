# observa-server

## OVERVIEW

`observa-server` is the HTTP dashboard and REST API library for Observa. It is consumed by `observa-cli` and exposes Axum routes, SSE, Tera rendering, auth, chat, rate limiting, and background tasks.

## WHERE TO LOOK

| Task | File | Notes |
|---|---|---|
| Dashboard routes | `/var/home/a/code/observa/crates/observa-server/src/routes/mod.rs` | Full router, middleware, and Tera integration |
| REST API | `/var/home/a/code/observa/crates/observa-server/src/api.rs` | `/api/*` endpoints |
| Auth/token middleware | `/var/home/a/code/observa/crates/observa-server/src/auth.rs` | Dashboard and chat token auth |
| Chat handlers | `/var/home/a/code/observa/crates/observa-server/src/chat.rs` | Ask/stream and session logic |
| Background tasks | `/var/home/a/code/observa/crates/observa-server/src/background.rs` | Health checks and insights |
| Shared state | `/var/home/a/code/observa/crates/observa-server/src/state.rs` | `AppState` |
| Storage seams | `/var/home/a/code/observa/crates/observa-server/src/store.rs` | `MetricStore` and `ChatStore` traits |
| Rate limiting | `/var/home/a/code/observa/crates/observa-server/src/rate_limit.rs` | Request throttling |
| Trusted-path sandbox | `/var/home/a/code/observa/crates/observa-server/src/tpe.rs` | Sandbox execution |
| LLM resolution | `/var/home/a/code/observa/crates/observa-server/src/llm.rs` | LLM fallback and resolution |

## CONVENTIONS

No server-specific conventions differ from the workspace defaults.

## ANTI-PATTERNS

- CSP in `src/routes/mod.rs` allows `'unsafe-inline'` and external resources such as `unpkg.com` and Google Fonts.
- Blocking synchronous I/O (`std::fs` and `std::process::Command`) runs inside async paths in `src/tpe.rs`.
- A production `.expect()` remains in `src/insight.rs`.

## COMMANDS

```bash
# Run server tests
cargo test -p observa-server

# Lint with warnings as errors
cargo clippy -p observa-server -- -D warnings
```
