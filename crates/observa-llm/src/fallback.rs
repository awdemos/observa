use observa_shared::{format_bytes, AiServerKind, ChatMessage, Result, Role};

const GREETING: &str = "Hello. I'm Observa. Ask me about CPU, memory, disks, networks, processes, logs, security alerts, or detected AI servers.";

const UPGRADE_HINT: &str = "Configure an OpenAI-compatible LLM API (OBSERVA__LLM_API_BASE, OBSERVA__LLM_API_KEY, OBSERVA__LLM_MODEL) for richer, model-driven answers.";

/// A small rule-based responder used when no external LLM is configured.
/// It gives terse, context-aware answers from the injected metrics and logs.
#[derive(Debug, Clone, Default)]
pub struct FallbackResponder;

impl FallbackResponder {
    pub fn new() -> Self {
        Self
    }

    /// Produce an assistant reply from a prompt that includes system instructions,
    /// metric context, log context, and the user's question.
    pub async fn complete(&self, messages: &[ChatMessage]) -> Result<ChatMessage> {
        let question = last_user_question(messages).unwrap_or_default().to_lowercase();
        let metrics = extract_metrics(messages);
        let logs = extract_logs(messages);
        let ai_servers = extract_ai_servers(messages);
        let processes = extract_processes(messages);

        let reply = if is_greeting(&question) {
            GREETING.to_string()
        } else if question.contains("cpu") || question.contains("processor") {
            format_cpu_answer(&metrics)
        } else if question.contains("memory") || question.contains("ram") {
            format_memory_answer(&metrics)
        } else if question.contains("disk") || question.contains("storage") {
            format_disk_answer(&metrics)
        } else if question.contains("network") || question.contains("interface") || question.contains("net") {
            format_network_answer(&metrics)
        } else if question.contains("ai server")
            || question.contains("ai-server")
            || question.contains("model server")
            || question.contains("inference")
            || question.contains("ollama")
            || question.contains("llama")
            || question.contains("vllm")
            || question.contains("localai")
        {
            format_ai_server_answer(&ai_servers)
        } else if question.contains("process")
            || question.contains("top")
            || question.contains("running")
            || (question.contains("what") && question.contains("running"))
        {
            format_process_answer(&processes, &metrics)
        } else if question.contains("log") || question.contains("error") || question.contains("warn") {
            format_log_answer(&logs)
        } else if question.contains("security") || question.contains("alert") || question.contains("attack") {
            format_security_answer(&logs)
        } else {
            format!("I can answer questions about CPU, memory, disks, networks, processes, logs, security, and detected AI servers. {}", UPGRADE_HINT)
        };

        Ok(ChatMessage {
            role: Role::Assistant,
            content: reply,
        })
    }
}

fn is_greeting(text: &str) -> bool {
    let t = text.trim();
    t == "hello"
        || t == "hi"
        || t == "hey"
        || t == "hello!"
        || t == "hi!"
        || t.starts_with("hello ")
        || t.starts_with("hi ")
}

fn last_user_question(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User && !m.content.starts_with("Latest metrics:") && !m.content.starts_with("Recent log ["))
        .map(|m| m.content.clone())
}

#[derive(Debug, Default)]
struct ParsedMetrics {
    cpu_pct: Option<f32>,
    memory_used: Option<u64>,
    memory_total: Option<u64>,
    disk_count: Option<usize>,
    network_count: Option<usize>,
    process_count: Option<usize>,
}

#[derive(Debug, Default)]
struct ParsedProcess {
    name: String,
    cpu_pct: f32,
    memory_bytes: u64,
}

#[derive(Debug, Default)]
struct ParsedAiServer {
    name: String,
    kind: AiServerKind,
}

fn extract_metrics(messages: &[ChatMessage]) -> ParsedMetrics {
    let mut out = ParsedMetrics::default();
    for msg in messages {
        if msg.role != Role::User {
            continue;
        }
        if let Some(rest) = msg.content.strip_prefix("Latest metrics: CPU ") {
            let mut parts = rest.splitn(2, '%');
            if let Some(cpu_str) = parts.next() {
                out.cpu_pct = cpu_str.parse().ok();
            }
            if let Some(after_cpu) = parts.next() {
                let parts: Vec<&str> = after_cpu.split_whitespace().collect();
                for (i, &p) in parts.iter().enumerate() {
                    if p == "memory" && i + 1 < parts.len() {
                        let mem_parts: Vec<&str> = parts[i + 1].split('/').collect();
                        if mem_parts.len() == 2 {
                            out.memory_used = mem_parts[0].parse().ok();
                            out.memory_total = mem_parts[1].parse().ok();
                        }
                    }
                    if p == "disks," && i + 1 < parts.len() {
                        out.disk_count = parts[i + 1].parse().ok();
                    }
                    if p == "networks," && i + 1 < parts.len() {
                        out.network_count = parts[i + 1].parse().ok();
                    }
                    if p == "processes." && i > 0 {
                        out.process_count = parts[i - 1].parse().ok();
                    }
                }
            }
        }
    }
    out
}

fn extract_processes(messages: &[ChatMessage]) -> Vec<ParsedProcess> {
    let mut out = Vec::new();
    for msg in messages {
        if msg.role != Role::User {
            continue;
        }
        // Extract from metric context: "Top processes: opencode (12.3% CPU, 1.2 GiB), ..."
        if let Some(rest) = msg.content.split("Top processes: ").nth(1) {
            if let Some(end) = rest.find(". AI servers:") {
                for part in rest[..end].split(", ") {
                    if let Some((name, tail)) = part.split_once(" (") {
                        let mut p = ParsedProcess {
                            name: name.to_string(),
                            ..Default::default()
                        };
                        if let Some(cpu_end) = tail.find("% CPU") {
                            p.cpu_pct = tail[..cpu_end].parse().unwrap_or(0.0);
                        }
                        if let Some(mem_start) = tail.find(", ") {
                            p.memory_bytes = parse_bytes(tail[mem_start + 2..].trim_end_matches(")")).unwrap_or(0);
                        }
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

fn extract_ai_servers(messages: &[ChatMessage]) -> Vec<ParsedAiServer> {
    let mut out = Vec::new();
    for msg in messages {
        if msg.role != Role::User {
            continue;
        }
        // Extract from metric context: "AI servers: ollama [Ollama], ..."
        for segment in msg.content.split("AI servers: ").skip(1) {
            let segment = segment.split('.').next().unwrap_or(segment);
            for part in segment.split(", ") {
                if let Some((name, kind_str)) = part.split_once(" [") {
                    if let Some(kind_end) = kind_str.find("]") {
                        let kind = match kind_str[..kind_end].to_lowercase().as_str() {
                            "vllm" => AiServerKind::Vllm,
                            "ollama" => AiServerKind::Ollama,
                            "triton" => AiServerKind::Triton,
                            "openai" => AiServerKind::OpenAi,
                            "sglang" => AiServerKind::Sglang,
                            "llamacpp" | "llama.cpp" => AiServerKind::LlamaCpp,
                            "exllama" | "exllamav2" => AiServerKind::ExllamaV2,
                            "kobold" | "koboldcpp" => AiServerKind::KoboldCpp,
                            "tabby" | "tabbyapi" => AiServerKind::TabbyApi,
                            "lmstudio" => AiServerKind::LmStudio,
                            "tgi" | "textgenerationinference" => AiServerKind::TextGenerationInference,
                            "generic" => AiServerKind::Generic,
                            _ => AiServerKind::Generic,
                        };
                        out.push(ParsedAiServer {
                            name: name.to_string(),
                            kind,
                        });
                    }
                }
            }
        }
        // Also handle messages explicitly about AI servers.
        for line in msg.content.lines() {
            if line.starts_with("AI server:") {
                let rest = line.strip_prefix("AI server:").unwrap_or("").trim();
                if let Some((name, kind_str)) = rest.split_once(" [") {
                    if let Some(kind_end) = kind_str.find("]") {
                        let kind = parse_ai_kind(&kind_str[..kind_end]);
                        out.push(ParsedAiServer {
                            name: name.to_string(),
                            kind,
                        });
                    }
                }
            }
        }
    }
    out
}

fn parse_ai_kind(s: &str) -> AiServerKind {
    match s.to_lowercase().as_str() {
        "vllm" => AiServerKind::Vllm,
        "ollama" => AiServerKind::Ollama,
        "triton" => AiServerKind::Triton,
        "openai" => AiServerKind::OpenAi,
        "sglang" => AiServerKind::Sglang,
        "llamacpp" | "llama.cpp" => AiServerKind::LlamaCpp,
        "exllama" | "exllamav2" => AiServerKind::ExllamaV2,
        "kobold" | "koboldcpp" => AiServerKind::KoboldCpp,
        "tabby" | "tabbyapi" => AiServerKind::TabbyApi,
        "lmstudio" => AiServerKind::LmStudio,
        "tgi" | "textgenerationinference" => AiServerKind::TextGenerationInference,
        _ => AiServerKind::Generic,
    }
}

fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let mut split = s.split_whitespace();
    let value: f64 = split.next()?.parse().ok()?;
    let unit = split.next().unwrap_or("B");
    let multiplier = match unit.to_lowercase().as_str() {
        "b" => 1.0,
        "ki" | "kib" => 1024.0,
        "mi" | "mib" => 1024.0 * 1024.0,
        "gi" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "ti" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    Some((value * multiplier) as u64)
}

fn extract_logs(messages: &[ChatMessage]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m.role == Role::User && m.content.starts_with("Recent log ["))
        .map(|m| m.content.clone())
        .collect()
}

fn format_cpu_answer(m: &ParsedMetrics) -> String {
    match m.cpu_pct {
        Some(pct) => format!("CPU is at {:.1}% across {} processes.", pct, m.process_count.unwrap_or(0)),
        None => "No CPU metrics available yet.".to_string(),
    }
}

fn format_memory_answer(m: &ParsedMetrics) -> String {
    match (m.memory_used, m.memory_total) {
        (Some(used), Some(total)) => {
            let pct = if total == 0 { 0.0 } else { 100.0 * used as f64 / total as f64 };
            format!(
                "Memory: {} used of {} ({:.0}%).",
                format_bytes(used),
                format_bytes(total),
                pct,
            )
        }
        _ => "No memory metrics available yet.".to_string(),
    }
}

fn format_disk_answer(m: &ParsedMetrics) -> String {
    match m.disk_count {
        Some(n) => format!("{} disk(s) tracked. See the Metrics page for per-disk usage.", n),
        None => "No disk metrics available yet.".to_string(),
    }
}

fn format_network_answer(m: &ParsedMetrics) -> String {
    match m.network_count {
        Some(n) => format!("{} network interface(s) tracked. See the Network page for live throughput.", n),
        None => "No network metrics available yet.".to_string(),
    }
}

fn format_process_answer(processes: &[ParsedProcess], m: &ParsedMetrics) -> String {
    if processes.is_empty() {
        return match m.process_count {
            Some(n) => format!("Tracking {} top processes by CPU. The Processes page lists them.", n),
            None => "No process metrics available yet.".to_string(),
        };
    }
    let top: Vec<String> = processes
        .iter()
        .map(|p| format!("{} ({:.1}% CPU)", p.name, p.cpu_pct))
        .collect();
    format!("Top processes: {}.", top.join(", "))
}

fn format_ai_server_answer(ai_servers: &[ParsedAiServer]) -> String {
    if ai_servers.is_empty() {
        return "No AI model servers detected in the current snapshot.".to_string();
    }
    let list: Vec<String> = ai_servers
        .iter()
        .map(|a| format!("{} ({:?})", a.name, a.kind))
        .collect();
    format!("Detected AI servers: {}.", list.join(", "))
}

fn format_log_answer(logs: &[String]) -> String {
    if logs.is_empty() {
        return "No recent logs in context.".to_string();
    }
    let errors = logs.iter().filter(|l| l.contains("Error") || l.contains("Critical")).count();
    let warns = logs.iter().filter(|l| l.contains("Warn")).count();
    if errors > 0 || warns > 0 {
        format!(
            "Recent logs show {} error/critical and {} warning events out of {} total. See the Logs page for details.",
            errors,
            warns,
            logs.len(),
        )
    } else {
        format!("{} recent log events, no errors or warnings.", logs.len())
    }
}

fn format_security_answer(logs: &[String]) -> String {
    let security_logs: Vec<&String> = logs
        .iter()
        .filter(|l| {
            let lower = l.to_lowercase();
            lower.contains("security")
                || lower.contains("auth")
                || lower.contains("sudo")
                || lower.contains("ssh")
                || lower.contains("firewall")
                || lower.contains("iptables")
        })
        .collect();
    if security_logs.is_empty() {
        "No security alerts detected in recent logs.".to_string()
    } else {
        format!(
            "{} security-relevant event(s) in recent logs. See the Security page.",
            security_logs.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.to_string(),
        }
    }

    fn system() -> ChatMessage {
        ChatMessage {
            role: Role::System,
            content: "You are Observa.".to_string(),
        }
    }

    #[tokio::test]
    async fn answers_cpu_question() {
        let responder = FallbackResponder::new();
        let messages = vec![
            system(),
            user("Latest metrics: CPU 12.5%, memory 8000000000/16000000000 bytes, 2 disks, 3 networks, 42 processes."),
            user("how is cpu?"),
        ];
        let reply = responder.complete(&messages).await.unwrap();
        assert!(reply.content.contains("CPU is at 12.5%"));
        assert!(reply.role == Role::Assistant);
    }

    #[tokio::test]
    async fn greets_user() {
        let responder = FallbackResponder::new();
        let messages = vec![system(), user("hello")];
        let reply = responder.complete(&messages).await.unwrap();
        assert!(reply.content.contains("Hello. I'm Observa"));
    }

    #[tokio::test]
    async fn unknown_question_hints_at_configuration() {
        let responder = FallbackResponder::new();
        let messages = vec![system(), user("what is the meaning of life?")];
        let reply = responder.complete(&messages).await.unwrap();
        assert!(reply.content.contains("Configure an OpenAI-compatible LLM API"));
    }
}
