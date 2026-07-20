# observa-cli

## OVERVIEW

This crate is the executable entry point that composes and runs all observability subsystems.

## WHERE TO LOOK

| File | Role |
|---|---|
| `/var/home/a/code/observa/crates/observa-cli/src/main.rs` | Composes the collector, ingestor, database, cache, bus, and server, then starts the Axum dashboard. |
| `/var/home/a/code/observa/crates/observa-cli/Cargo.toml` | Defines the binary crate and its workspace-only dependencies. |

## COMMANDS

```bash
# Run the dashboard locally
cargo run -p observa-cli

# Run crate tests
cargo test -p observa-cli

# Lint with warnings as errors
cargo clippy -p observa-cli -- -D warnings
```

## NOTES

- This crate contains no library code. All reusable logic lives in sibling crates.
- The server address, log level, and feature flags come from `observa-config`.
- Use the workspace root `observa.toml` to change runtime settings.
