use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "observa")]
#[command(about = "Real-time system observability dashboard")]
pub struct Cli {
    #[arg(short, long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[arg(short, long, value_name = "ADDR", env = "OBSERVA_BIND")]
    pub bind: Option<String>,

    #[arg(long, value_name = "URL", env = "OBSERVA_DATABASE_URL")]
    pub database_url: Option<String>,

    #[arg(long, value_name = "URL", env = "OBSERVA_REDIS_URL")]
    pub redis_url: Option<String>,

    #[arg(long, value_name = "URL", env = "OBSERVA_LLM_API_BASE")]
    pub llm_api_base: Option<String>,

    #[arg(long, value_name = "MODEL", env = "OBSERVA_LLM_MODEL")]
    pub llm_model: Option<String>,

    #[arg(long, value_name = "KEY", env = "OBSERVA_LLM_API_KEY")]
    pub llm_api_key: Option<String>,

    /// Per-request timeout for LLM completion calls, in seconds (default: 60).
    /// Local models that take time to load may need 120-300 seconds.
    #[arg(long, value_name = "SECS", env = "OBSERVA_LLM_TIMEOUT_SECS")]
    pub llm_timeout_secs: Option<u64>,

    #[arg(long, value_name = "MS", env = "OBSERVA_SAMPLE_INTERVAL_MS")]
    pub sample_interval_ms: Option<u64>,

    #[arg(long, value_name = "SOURCE", env = "OBSERVA_LOG_SOURCE")]
    pub log_source: Option<String>,

    #[arg(long, value_name = "PATH", env = "OBSERVA_LOG_FILE")]
    pub log_file: Option<PathBuf>,

    #[arg(long, env = "OBSERVA_NO_TAIL")]
    pub no_tail: bool,

    #[arg(long, value_name = "DAYS", env = "OBSERVA_RETENTION_DAYS")]
    pub retention_days: Option<u64>,

    #[arg(long, env = "OBSERVA_COMPRESSION_DISABLED")]
    pub compression_disabled: bool,

    #[arg(long, value_name = "HOURS", env = "OBSERVA_VACUUM_INTERVAL_HOURS")]
    pub vacuum_interval_hours: Option<u64>,

    /// Bearer token required to access the web UI and API.
    ///
    /// When unset, the dashboard is publicly reachable (not recommended for
    /// any network with untrusted users).  Set a strong random value in
    /// production.
    #[arg(long, value_name = "TOKEN", env = "OBSERVA_DASHBOARD_TOKEN")]
    pub dashboard_token: Option<String>,
}

impl Cli {
    /// Returns true if any CLI field that has higher precedence than the config
    /// file has been explicitly provided. This is intentionally conservative:
    /// we do not count `no_tail` alone as an override.
    pub fn has_config_overrides(&self) -> bool {
        self.bind.is_some()
            || self.database_url.is_some()
            || self.redis_url.is_some()
            || self.llm_api_base.is_some()
            || self.llm_model.is_some()
            || self.llm_api_key.is_some()
            || self.sample_interval_ms.is_some()
            || self.log_source.is_some()
            || self.log_file.is_some()
            || self.retention_days.is_some()
            || self.vacuum_interval_hours.is_some()
            || self.compression_disabled
            || self.llm_timeout_secs.is_some()
            || self.dashboard_token.is_some()
    }
}
