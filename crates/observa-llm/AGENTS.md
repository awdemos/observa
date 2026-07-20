# observa-llm

## OVERVIEW

This crate provides an OpenAI-compatible chat client with completion streaming for the dashboard chat feature. It handles request building, streaming response parsing, and error conversion.

## WHERE TO LOOK

| File | Role |
|---|---|
| `/var/home/a/code/observa/crates/observa-llm/src/client.rs` | `LlmClient` implementation, request helpers, and streaming logic. |
| `/var/home/a/code/observa/crates/observa-llm/src/lib.rs` | Public re-exports and crate-level documentation. |

## ANTI-PATTERNS

One production `.expect()` remains in `/var/home/a/code/observa/crates/observa-llm/src/client.rs`. Replace it with proper error handling before treating this crate as fully hardened.

## COMMANDS

```bash
# Run crate tests
cargo test -p observa-llm

# Lint with warnings as errors
cargo clippy -p observa-llm -- -D warnings
```

## NOTES

- The client is generic over the OpenAI chat completions shape.
- Streaming uses server-sent events from the remote endpoint.
- Keep all API-specific types behind the crate public surface so callers do not depend on internal modules.
