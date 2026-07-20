# observa-shared

## OVERVIEW

Shared domain types and errors across the workspace. This crate should stay small, dependency-light, and free of business logic.

## WHERE TO LOOK

| Task | Location | Notes |
|---|---|---|
| Domain types and error enums | `/var/home/a/code/observa/crates/observa-shared/src/lib.rs` | Public types and errors used by multiple crates |

## COMMANDS

```bash
# Unit tests
cargo test -p observa-shared

# Lint (warnings are errors)
cargo clippy -p observa-shared -- -D warnings
```

## NOTES

- Keep this crate dependency-light. Other crates import it, so every new dependency ripples outward.
- Types here should be plain data, not services. Put behavior in the crate that owns the domain.
- Error enums should be additive and non-breaking; avoid changing variant names without checking callers.
