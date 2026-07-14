use std::path::PathBuf;

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

use observa_shared::{LogSource, ObservaError, Result};

use crate::cli::Cli;

const DEFAULT_RETENTION_DAYS: u64 = 7;

/// Return the platform-specific Observa data directory.
///
/// Used to locate the default SQLite database when no `database_url` is
/// configured. Falls back to the current directory if platform directories
/// cannot be resolved.
pub fn default_data_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "observa", "observa")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_database_url() -> String {
    let dir = default_data_dir();
    format!("sqlite://{}", dir.join("observa.db").to_string_lossy())
}

/// Internal struct matching the shape of the configuration file and env vars.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawConfig {
    database_url: Option<String>,
    redis_url: Option<String>,
    bind_addr: Option<String>,
    llm_api_base: Option<String>,
    llm_model: Option<String>,
    llm_api_key: Option<String>,
    llm_timeout_secs: Option<u64>,
    sample_interval_ms: Option<u64>,
    log_source: Option<String>,
    log_file: Option<PathBuf>,
    log_tail: Option<bool>,
    retention_days: Option<u64>,
    compression_enabled: Option<bool>,
    vacuum_interval_hours: Option<u64>,
    notifications_enabled: Option<bool>,
    notifications_webhook_url: Option<String>,
    log_page_size: Option<u64>,
    metric_history_minutes: Option<u64>,
    ai_server_endpoints: Option<Vec<String>>,
    ai_server_subnet_scan: Option<bool>,
    dashboard_token: Option<String>,
}

impl Default for RawConfig {
    fn default() -> Self {
        let default = Config::default();
        RawConfig {
            database_url: default.database_url,
            redis_url: default.redis_url,
            bind_addr: Some(default.bind_addr),
            llm_api_base: Some(default.llm_api_base),
            llm_model: Some(default.llm_model),
            llm_api_key: default.llm_api_key,
            llm_timeout_secs: Some(default.llm_timeout_secs),
            sample_interval_ms: Some(default.sample_interval_ms),
            log_source: Some(String::from("journald")),
            log_file: None,
            log_tail: Some(default.log_tail),
            retention_days: Some(default.retention_days),
            compression_enabled: Some(default.compression_enabled),
            vacuum_interval_hours: Some(default.vacuum_interval_hours),
            notifications_enabled: Some(default.notifications_enabled),
            notifications_webhook_url: default.notifications_webhook_url,
            log_page_size: Some(default.log_page_size),
            metric_history_minutes: Some(default.metric_history_minutes),
            ai_server_endpoints: Some(default.ai_server_endpoints.clone()),
            ai_server_subnet_scan: Some(default.ai_server_subnet_scan),
            dashboard_token: default.dashboard_token.clone(),
        }
    }
}

impl RawConfig {
    fn resolve(self) -> Result<Config> {
        let log_source = match self.log_source.as_deref() {
            Some("journald") | None => LogSource::Journald,
            Some("file") => {
                let path = self.log_file.ok_or_else(|| {
                    ObservaError::Config("log_source 'file' requires a log_file path".to_string())
                })?;
                LogSource::File { path }
            }
            Some(other) => {
                return Err(ObservaError::Config(format!(
                    "unknown log_source: {other}; expected 'journald' or 'file'"
                )));
            }
        };

        Ok(Config {
            database_url: self.database_url,
            redis_url: self.redis_url,
            bind_addr: self
                .bind_addr
                .unwrap_or_else(|| Config::default().bind_addr),
            llm_api_base: self
                .llm_api_base
                .unwrap_or_else(|| Config::default().llm_api_base),
            llm_model: self
                .llm_model
                .unwrap_or_else(|| Config::default().llm_model),
            llm_api_key: self.llm_api_key,
            llm_timeout_secs: self
                .llm_timeout_secs
                .unwrap_or_else(|| Config::default().llm_timeout_secs),
            sample_interval_ms: self
                .sample_interval_ms
                .unwrap_or_else(|| Config::default().sample_interval_ms),
            log_source,
            log_tail: self.log_tail.unwrap_or_else(|| Config::default().log_tail),
            retention_days: self
                .retention_days
                .unwrap_or_else(|| Config::default().retention_days),
            compression_enabled: self
                .compression_enabled
                .unwrap_or_else(|| Config::default().compression_enabled),
            vacuum_interval_hours: self
                .vacuum_interval_hours
                .unwrap_or_else(|| Config::default().vacuum_interval_hours),
            notifications_enabled: self
                .notifications_enabled
                .unwrap_or_else(|| Config::default().notifications_enabled),
            notifications_webhook_url: self.notifications_webhook_url,
            log_page_size: self
                .log_page_size
                .unwrap_or_else(|| Config::default().log_page_size),
            metric_history_minutes: self
                .metric_history_minutes
                .unwrap_or_else(|| Config::default().metric_history_minutes),
            ai_server_endpoints: self
                .ai_server_endpoints
                .unwrap_or_else(|| Config::default().ai_server_endpoints.clone()),
            ai_server_subnet_scan: self
                .ai_server_subnet_scan
                .unwrap_or(Config::default().ai_server_subnet_scan),
            dashboard_token: self.dashboard_token,
        })
    }
}

#[derive(Debug, Clone, Serialize, Eq, PartialEq)]
pub struct Config {
    pub database_url: Option<String>,
    pub redis_url: Option<String>,
    pub bind_addr: String,
    pub llm_api_base: String,
    pub llm_model: String,
    pub llm_api_key: Option<String>,
    pub llm_timeout_secs: u64,
    pub sample_interval_ms: u64,
    pub log_source: LogSource,
    pub log_tail: bool,
    pub retention_days: u64,
    pub compression_enabled: bool,
    pub vacuum_interval_hours: u64,
    pub notifications_enabled: bool,
    pub notifications_webhook_url: Option<String>,
    pub log_page_size: u64,
    pub metric_history_minutes: u64,
    pub ai_server_endpoints: Vec<String>,
    pub ai_server_subnet_scan: bool,
    pub dashboard_token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database_url: Some(default_database_url()),
            redis_url: None,
            bind_addr: String::from("127.0.0.1:3000"),
            llm_api_base: String::from("http://localhost:8080/v1"),
            llm_model: String::from("llama"),
            llm_api_key: None,
            llm_timeout_secs: 120,
            sample_interval_ms: 2000,
            log_source: LogSource::Journald,
            log_tail: true,
            retention_days: DEFAULT_RETENTION_DAYS,
            compression_enabled: true,
            vacuum_interval_hours: 24,
            notifications_enabled: false,
            notifications_webhook_url: None,
            log_page_size: 50,
            metric_history_minutes: 60,
            ai_server_endpoints: default_ai_server_endpoints(),
            ai_server_subnet_scan: false,
            dashboard_token: None,
        }
    }
}

fn default_ai_server_endpoints() -> Vec<String> {
    vec![
        // Common Docker Compose service names for inference engines.
        String::from("llama-server:8080"),
        String::from("ollama:11434"),
        String::from("vllm:8000"),
        String::from("triton:8000"),
        String::from("sglang:30000"),
        String::from("tabbyapi:5000"),
        String::from("lmstudio:1234"),
        // Host loopback aliases when running outside a container or in host network mode.
        String::from("127.0.0.1:8080"),
        String::from("127.0.0.1:18080"),
        String::from("127.0.0.1:11434"),
        String::from("127.0.0.1:8000"),
        String::from("127.0.0.1:5000"),
        String::from("127.0.0.1:30000"),
        String::from("127.0.0.1:1234"),
    ]
}

impl Config {
    pub fn load(cli: &Cli) -> Result<Self> {
        dotenvy::dotenv().ok();

        let file_path = cli
            .config
            .clone()
            .or_else(|| Some(PathBuf::from("observa.toml")))
            .filter(|p| p.exists());

        let mut figment = Figment::from(Serialized::defaults(RawConfig::default()))
            .merge(Env::prefixed("OBSERVA_").split("__"));

        if let Some(path) = file_path {
            figment = figment.merge(Toml::file(path));
        }

        let mut raw: RawConfig = figment
            .extract()
            .map_err(|e| ObservaError::Config(e.to_string()))?;

        if let Some(bind) = &cli.bind {
            raw.bind_addr = Some(bind.clone());
        }
        if let Some(database_url) = &cli.database_url {
            raw.database_url = Some(database_url.clone());
        }
        if let Some(redis_url) = &cli.redis_url {
            raw.redis_url = Some(redis_url.clone());
        }
        if let Some(llm_api_base) = &cli.llm_api_base {
            raw.llm_api_base = Some(llm_api_base.clone());
        }
        if let Some(llm_model) = &cli.llm_model {
            raw.llm_model = Some(llm_model.clone());
        }
        if let Some(llm_api_key) = &cli.llm_api_key {
            raw.llm_api_key = Some(llm_api_key.clone());
        }
        if let Some(llm_timeout_secs) = cli.llm_timeout_secs {
            raw.llm_timeout_secs = Some(llm_timeout_secs);
        }
        if let Some(sample_interval_ms) = cli.sample_interval_ms {
            raw.sample_interval_ms = Some(sample_interval_ms);
        }
        if let Some(log_source) = &cli.log_source {
            raw.log_source = Some(log_source.clone());
        }
        if let Some(log_file) = &cli.log_file {
            raw.log_file = Some(log_file.clone());
        }
        if cli.no_tail {
            raw.log_tail = Some(false);
        }
        if let Some(retention_days) = cli.retention_days {
            raw.retention_days = Some(retention_days);
        }
        if cli.compression_disabled {
            raw.compression_enabled = Some(false);
        }
        if let Some(vacuum_interval_hours) = cli.vacuum_interval_hours {
            raw.vacuum_interval_hours = Some(vacuum_interval_hours);
        }
        if let Some(dashboard_token) = &cli.dashboard_token {
            raw.dashboard_token = Some(dashboard_token.clone());
        }

        raw.resolve()
    }
}
