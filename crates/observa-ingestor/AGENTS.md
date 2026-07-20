# observa-ingestor

## OVERVIEW

Log and journal ingestion crate. Reads system logs, service journals, and other line-oriented sources and turns them into structured events for the dashboard.

## WHERE TO LOOK

| Task | Location | Notes |
|---|---|---|
| Reader logic | `/var/home/a/code/observa/crates/observa-ingestor/src/reader.rs` | Main line reader and parsing loop |
| Public surface | `/var/home/a/code/observa/crates/observa-ingestor/src/lib.rs` | Re-exports the crate's API |

## ANTI-PATTERNS

There is one production `.expect()` call in `/var/home/a/code/observa/crates/observa-ingestor/src/reader.rs`. Treat it as a known hazard; do not add new ones nearby.

## COMMANDS

```bash
# Unit tests
cargo test -p observa-ingestor

# Lint (warnings are errors)
cargo clippy -p observa-ingestor -- -D warnings
```

## NOTES

- This crate has no binary target; it is a library consumed by `observa-cli`.
- Keep parsing focused on timestamp, unit, message, and severity.
- Prefer streaming over collecting whole files into memory.
