use std::time::Duration;

use observa_shared::{format_bytes, ChatMessage, HealthStatus, LogEvent, MetricSnapshot, Result, Role, Severity};

use crate::llm;
use crate::state::AppState;

const LLM_INSIGHT_TIMEOUT: Duration = Duration::from_secs(120);

/// Generate a terse system-health insight from recent metrics and logs.
///
/// The returned string is already stripped of any visible reasoning chain and
/// classified by [`classify_health`]. This is the only place that knows how to
/// compose the LLM prompt for digest generation, so changes to the prompt or
/// classification thresholds live here rather than in the background loop.
pub async fn generate(state: &AppState, metrics: &[MetricSnapshot], logs: &[LogEvent]) -> Result<String> {
    let (system_text, user_text) = build_insight_prompt(metrics, logs);
    let prompt = vec![
        ChatMessage {
            role: Role::System,
            content: system_text,
        },
        ChatMessage {
            role: Role::User,
            content: user_text,
        },
    ];
    let reply = llm::complete_with_fallback(state, prompt, Some(LLM_INSIGHT_TIMEOUT)).await?;
    Ok(reply.content)
}

/// Build a short insight sentence from metrics and logs without calling an LLM.
///
/// Used when no external LLM is configured so the dashboard still shows a
/// meaningful, real-time summary instead of a configuration hint.
pub fn generate_local(metrics: &[MetricSnapshot], logs: &[LogEvent]) -> String {
    if metrics.is_empty() {
        return "No metrics collected yet. Insight will appear once the collector runs.".to_string();
    }

    let latest = metrics.last().expect("metrics non-empty");
    let cpu = latest.cpu.usage_percent;
    let memory_pct = if latest.memory.total_bytes == 0 {
        0.0
    } else {
        100.0 * latest.memory.used_bytes as f64 / latest.memory.total_bytes as f64
    };
    let memory_used = format_bytes(latest.memory.used_bytes);
    let memory_total = format_bytes(latest.memory.total_bytes);

    let error_logs = logs
        .iter()
        .filter(|l| matches!(l.severity, Severity::Error | Severity::Critical))
        .count();
    let warn_logs = logs.iter().filter(|l| l.severity == Severity::Warn).count();

    let mut clauses = Vec::new();
    clauses.push(format!("CPU {:.1}%, memory {:.0}%", cpu, memory_pct));
    if !latest.ai_servers.is_empty() {
        clauses.push(format!("{} AI server(s)", latest.ai_servers.len()));
    }
    if error_logs > 0 {
        clauses.push(format!("{} errors", error_logs));
    } else if warn_logs > 0 {
        clauses.push(format!("{} warnings", warn_logs));
    } else {
        clauses.push("logs quiet".to_string());
    }

    if cpu >= 90.0 || memory_pct >= 90.0 || error_logs > 0 {
        format!(
            "System under pressure: {} ({}/{} memory used).",
            clauses.join(", "),
            memory_used,
            memory_total
        )
    } else {
        format!(
            "System looks stable: {} ({}/{} memory used).",
            clauses.join(", "),
            memory_used,
            memory_total
        )
    }
}

/// Classify a free-text insight summary into a coarse health status.
pub fn classify_health(summary: &str) -> HealthStatus {
    let lower = summary.to_lowercase();
    if lower.contains("critical") || lower.contains("failure") || lower.contains("unhealthy") {
        HealthStatus::Unhealthy
    } else if lower.contains("warning")
        || lower.contains("high")
        || lower.contains("degraded")
        || lower.contains("elevated")
    {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    }
}

fn build_insight_prompt(metrics: &[MetricSnapshot], logs: &[LogEvent]) -> (String, String) {
    let system = "You are Observa, a system monitoring assistant. \
Summarize the health of this system in one terse sentence (max 140 chars). \
Mention only anomalies or notable trends. If nothing is wrong, say so."
        .to_string();

    let mut lines = Vec::new();

    if let Some(latest) = metrics.last() {
        lines.push(format!(
            "Latest metrics: CPU {:.1}%, memory {}/{} bytes, {} disks, {} networks, {} processes.",
            latest.cpu.usage_percent,
            latest.memory.used_bytes,
            latest.memory.total_bytes,
            latest.disks.len(),
            latest.networks.len(),
            latest.processes.len()
        ));
    }

    let errors = logs
        .iter()
        .filter(|l| matches!(l.severity, observa_shared::Severity::Error | observa_shared::Severity::Critical))
        .count();
    let warns = logs.iter().filter(|l| l.severity == observa_shared::Severity::Warn).count();
    lines.push(format!(
        "Recent logs: {} total, {} errors/critical, {} warnings.",
        logs.len(),
        errors,
        warns
    ));

    (system, lines.join("\n"))
}
