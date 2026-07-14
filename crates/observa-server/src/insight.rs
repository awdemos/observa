use std::time::Duration;

use observa_shared::{format_bytes, ChatMessage, HealthStatus, LogEvent, MetricSnapshot, Result, Role, Severity};

use crate::llm;
use crate::state::AppState;

const LLM_INSIGHT_TIMEOUT: Duration = Duration::from_secs(120);

/// Generate a terse system-health insight from recent metrics and logs.
///
/// The returned string is stored verbatim; health and alert severity are
/// derived from real metrics and logs in the background digest loop so that
/// LLM wording (e.g. "no critical issues") cannot skew classification.
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

    let critical_logs = logs.iter().filter(|l| l.severity == Severity::Critical).count();
    let error_logs = logs.iter().filter(|l| l.severity == Severity::Error).count();
    let warn_logs = logs.iter().filter(|l| l.severity == Severity::Warn).count();

    let mut clauses = Vec::new();
    clauses.push(format!("CPU {:.1}%, memory {:.0}%", cpu, memory_pct));
    if !latest.ai_servers.is_empty() {
        clauses.push(format!("{} AI server(s)", latest.ai_servers.len()));
    }
    if critical_logs > 0 {
        clauses.push(format!("{} critical issue{}", critical_logs, if critical_logs == 1 { "" } else { "s" }));
    } else if error_logs > 0 {
        clauses.push(format!("{} error{}", error_logs, if error_logs == 1 { "" } else { "s" }));
    } else if warn_logs > 0 {
        clauses.push(format!("{} warning{}", warn_logs, if warn_logs == 1 { "" } else { "s" }));
    } else {
        clauses.push("logs quiet".to_string());
    }

    if cpu >= 90.0 || memory_pct >= 90.0 || critical_logs > 0 || error_logs > 0 {
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
    } else if lower.contains("under pressure") || lower.contains("degraded") {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    }
}

fn build_insight_prompt(metrics: &[MetricSnapshot], logs: &[LogEvent]) -> (String, String) {
    let system = "You are Observa, a system monitoring assistant. \
Summarize the health of this system in one terse sentence (max 140 chars). \
Mention only real anomalies or notable trends. \
Do not use words like 'warning' or 'critical' for normal or moderate values. \
Only say 'critical' if there are critical-severity log events or resource usage above 90%."
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
