# observa-db

## OVERVIEW

`observa-db` owns the SQLite persistence layer and schema migrations for Observa. It provides a single pooled connection wrapper (`Db`) and per-domain storage modules for metrics, logs, chat sessions, and security alerts.

## WHERE TO LOOK

| Task | Location | Notes |
|---|---|---|
| Connection pool and DB setup | `/var/home/a/code/observa/crates/observa-db/src/pool.rs` | `Db` struct and constructor used by the main binary |
| Metric persistence | `/var/home/a/code/observa/crates/observa-db/src/metrics.rs` | Compressed metric storage and queries |
| Log persistence | `/var/home/a/code/observa/crates/observa-db/src/logs.rs` | Log ingestion storage |
| Chat sessions and messages | `/var/home/a/code/observa/crates/observa-db/src/chat.rs` | Stores session metadata and message history |
| Security alerts | `/var/home/a/code/observa/crates/observa-db/src/security.rs` | Alert chain with hashes |
| Database schema | `/var/home/a/code/observa/crates/observa-db/migrations/20260707000001_initial.sql` | Initial migration defining tables and indexes |

## COMMANDS

```bash
# Run crate tests
cargo test -p observa-db

# Run clippy with warnings as errors
cargo clippy -p observa-db -- -D warnings
```
