use observa_shared::{LogEvent, Result, SecurityAlert, Severity};

use crate::state::AppState;

const MAX_SEVERITY_LEN: usize = 20;
pub(crate) const MAX_LOG_QUERY_LEN: usize = 200;
const KNOWN_SEVERITIES: &[&str] = &["debug", "info", "warn", "error", "critical"];

fn clean_severity_part(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_SEVERITY_LEN {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if KNOWN_SEVERITIES.contains(&lower.as_str()) {
        Some(lower)
    } else {
        None
    }
}

#[derive(Debug, Default)]
pub struct LogFilter {
    pub q: Option<String>,
    pub severity: Vec<String>,
    pub page: usize,
    pub page_size: usize,
}

impl<'de> serde::Deserialize<'de> for LogFilter {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FilterVisitor;

        fn parse_usize(s: &str) -> Option<usize> {
            s.parse::<usize>().ok()
        }

        impl<'de> serde::de::Visitor<'de> for FilterVisitor {
            type Value = LogFilter;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("query parameters")
            }

            fn visit_map<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut q: Option<String> = None;
                let mut severity = Vec::new();
                let mut page: Option<usize> = None;
                let mut page_size: Option<usize> = None;
                while let Some(key) = access.next_key::<String>()? {
                    match key.as_str() {
                        "q" => {
                            q = access.next_value::<Option<String>>()?;
                        }
                        "severity" => {
                            let value: String = access.next_value()?;
                            severity.extend(value.split(',').filter_map(clean_severity_part));
                        }
                        "page" => {
                            let value: String = access.next_value()?;
                            page = parse_usize(&value);
                        }
                        "page_size" => {
                            let value: String = access.next_value()?;
                            page_size = parse_usize(&value);
                        }
                        _ => {
                            let _ = access.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                let q = q.as_ref().map(|s| {
                    if s.len() > MAX_LOG_QUERY_LEN {
                        s[..MAX_LOG_QUERY_LEN].to_string()
                    } else {
                        s.clone()
                    }
                });
                Ok(LogFilter {
                    q,
                    severity,
                    page: page.unwrap_or(0),
                    page_size: page_size.unwrap_or(0),
                })
            }
        }

        deserializer.deserialize_map(FilterVisitor)
    }
}

#[derive(Debug, Default)]
pub struct SecurityFilter {
    pub severity: Vec<String>,
}

impl<'de> serde::Deserialize<'de> for SecurityFilter {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FilterVisitor;

        impl<'de> serde::de::Visitor<'de> for FilterVisitor {
            type Value = SecurityFilter;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("query parameters")
            }

            fn visit_map<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut severity = Vec::new();
                while let Some(key) = access.next_key::<String>()? {
                    if key.as_str() == "severity" {
                        let value: String = access.next_value()?;
                        severity.extend(value.split(',').filter_map(clean_severity_part));
                    } else {
                        let _ = access.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(SecurityFilter { severity })
            }
        }

        deserializer.deserialize_map(FilterVisitor)
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct MetricRange {
    #[serde(default)]
    pub range: String,
}

impl MetricRange {
    pub fn minutes(&self) -> u64 {
        match self.range.as_str() {
            "15m" => 15,
            "1h" => 60,
            "6h" => 360,
            "24h" => 1440,
            "7d" => 10080,
            _ => 60,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct ChatQuery {
    pub session_id: Option<uuid::Uuid>,
    pub owner_token: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct SeverityCount {
    pub severity: String,
    pub class: String,
    pub count: usize,
}

pub fn parse_severity_filter(values: &[String]) -> Vec<Severity> {
    values
        .iter()
        .filter_map(|s| match s.as_str() {
            "debug" => Some(Severity::Debug),
            "info" => Some(Severity::Info),
            "warn" => Some(Severity::Warn),
            "error" => Some(Severity::Error),
            "critical" => Some(Severity::Critical),
            _ => None,
        })
        .collect()
}

pub async fn filtered_logs(
    state: &AppState,
    filter: &LogFilter,
    limit: usize,
) -> Result<Vec<LogEvent>> {
    let severities = parse_severity_filter(&filter.severity);
    state
        .store
        .search_logs(filter.q.as_deref(), &severities, limit)
        .await
}

pub async fn filtered_security_alerts(
    state: &AppState,
    severities: &[Severity],
    limit: usize,
) -> Result<Vec<SecurityAlert>> {
    let alerts = state.store.security_alerts(limit).await?;
    if severities.is_empty() {
        return Ok(alerts);
    }
    Ok(alerts
        .into_iter()
        .filter(|a| severities.contains(&a.severity))
        .collect())
}
