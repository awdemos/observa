use std::io::Write;

use clap::Parser;
use observa_config::{Cli, Config, LogSource};

#[test]
fn cli_overrides_config_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("observa.toml");

    let mut file = std::fs::File::create(&config_path).expect("create config");
    file.write_all(
        br#"
bind_addr = "127.0.0.1:4000"
llm_model = "gpt-3.5-turbo"
sample_interval_ms = 5000
"#,
    )
    .expect("write config");

    let cli = Cli::parse_from([
        "observa",
        "--config",
        config_path.to_str().unwrap(),
        "--bind",
        "127.0.0.1:9000",
    ]);

    let config = Config::load(&cli).expect("load config");
    assert_eq!(config.bind_addr, "127.0.0.1:9000");
    assert_eq!(config.sample_interval_ms, 5000);
    assert_eq!(config.llm_model, "gpt-3.5-turbo");
    assert_eq!(config.log_source, LogSource::Journald);
    assert!(config.log_tail);
}

#[test]
fn log_source_file_requires_path() {
    let cli = Cli::parse_from(["observa", "--log-source", "file"]);
    let error = Config::load(&cli).expect_err("config should fail");
    assert!(format!("{error}").contains("log_file path"));
}

#[test]
fn defaults_when_no_config_present() {
    let cli = Cli::parse_from(["observa"]);
    let config = Config::load(&cli).expect("load config");
    assert_eq!(config.bind_addr, "127.0.0.1:3000");
    assert_eq!(config.llm_model, "llama");
    assert_eq!(config.llm_api_base, "http://localhost:8080/v1");
    assert!(config.database_url.is_some());
    assert!(config
        .database_url
        .as_ref()
        .unwrap()
        .ends_with("observa.db"));
    assert!(config.redis_url.is_none());
    assert_eq!(config.log_source, LogSource::Journald);
}
