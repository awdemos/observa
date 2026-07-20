# observa-bus

## OVERVIEW

Server-Sent Events (SSE) broadcast bus. Handles fan-out of events to connected dashboard clients over persistent HTTP streams.

## WHERE TO LOOK

| Task | Location | Notes |
|---|---|---|
| Bus struct and broadcast | `/var/home/a/code/observa/crates/observa-bus/src/lib.rs` | Defines `Bus`, channels, and subscriber management |

## COMMANDS

```bash
# Unit tests
cargo test -p observa-bus

# Lint (warnings are errors)
cargo clippy -p observa-bus -- -D warnings
```

## NOTES

- Backpressure matters here. Dropping slow subscribers is preferred over blocking the whole bus.
- The bus is shared across the server via an `Arc` and is typically stored in `AppState`.
- Keep the payload format stable; HTMX partials depend on the shape of emitted events.
