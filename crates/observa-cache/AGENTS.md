# observa-cache

## OVERVIEW

Redis wrapper with an in-memory fallback. Provides a cache abstraction so the rest of the workspace can rely on cache semantics without caring whether Redis is present.

## WHERE TO LOOK

| Task | Location | Notes |
|---|---|---|
| Cache client and fallback | `/var/home/a/code/observa/crates/observa-cache/src/lib.rs` | Defines the client, fallback logic, and public API |

## COMMANDS

```bash
# Unit tests
cargo test -p observa-cache

# Lint (warnings are errors)
cargo clippy -p observa-cache -- -D warnings
```

## NOTES

- The fallback path exists to keep local development simple.
- Any change to cache behavior must keep both the Redis and fallback paths consistent.
- Do not introduce external HTTP client code here; this crate only speaks Redis or memory.
