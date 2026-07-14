pub mod parser;
pub mod reader;

pub use observa_shared::LogSource;
pub use parser::{parse_fallback_line, parse_journalctl_json};
pub use reader::{spawn_ingestor, IngestorOpts};
