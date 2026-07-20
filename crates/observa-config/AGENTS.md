# observa-config

## OVERVIEW

This crate handles configuration parsing, command-line arguments, tracing setup, and graceful shutdown signals. It turns the `observa.toml` file and `OBSERVA_*` environment variables into a typed config the rest of the workspace consumes.

## WHERE TO LOOK

| File | Role |
|---|---|
| `/var/home/a/code/observa/crates/observa-config/src/lib.rs` | Config structs, default values, and environment override logic. |
| `/var/home/a/code/observa/crates/observa-config/src/cli.rs` | CLI flag definitions and parsing. |
| `/var/home/a/code/observa/crates/observa-config/src/shutdown.rs` | Graceful shutdown helper and signal handling. |

## COMMANDS

```bash
# Run crate tests
cargo test -p observa-config

# Lint with warnings as errors
cargo clippy -p observa-config -- -D warnings
```

## NOTES

- Keep config values serializable and well-documented so the CLI crate can build cleanly.
- Environment overrides should follow the `OBSERVA_SECTION_KEY` pattern.
- Tracing subscriber setup belongs here, not in `observa-cli`.
