# observa-collector

## OVERVIEW

The collector crate runs background sampling and local AI server discovery. It produces metric snapshots and publishes service announcements for the dashboard.

## WHERE TO LOOK

| Task | Location | Notes |
|---|---|---|
| Background sampling loop | `/var/home/a/code/observa/crates/observa-collector/src/collector.rs` | `spawn_collector` drives the interval ticker and emits snapshots |
| Metric normalization | `/var/home/a/code/observa/crates/observa-collector/src/normalize.rs` | Builds `MetricSnapshot`; includes host memory fallback and GPU discovery |
| GPU discovery | `/var/home/a/code/observa/crates/observa-collector/src/normalize.rs` | Prefers `nvidia-smi`, falls back to `/sys/class/drm/PCI` scanning |
| AI server discovery | `/var/home/a/code/observa/crates/observa-collector/src/ai_scanner.rs` | Probes local subnet for Ollama, llama.cpp, and vLLM endpoints |
| Crate public surface | `/var/home/a/code/observa/crates/observa-collector/src/lib.rs` | Re-exports collector and scanner types |

## ANTI-PATTERNS

- Blocking synchronous I/O (`std::fs`, `std::process::Command`) inside async paths. `normalize.rs` calls `Command::output` and reads sysfs files directly; isolate this and move it to `spawn_blocking` or a dedicated thread.
- Tight polling loops without backpressure. The collector interval should wait for the previous sample to finish before scheduling the next tick.
- Hard-coding subnet scan ranges or service ports without a config override. Read these values from `observa-config`.

## COMMANDS

```bash
# Run collector tests
cargo test -p observa-collector

# Run collector clippy with warnings as errors
cargo clippy -p observa-collector -- -D warnings
```
