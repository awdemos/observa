pub mod cli;
pub mod config;
pub mod shutdown;

pub use cli::Cli;
pub use config::Config;
pub use observa_shared::LogSource;
pub use shutdown::shutdown_signal;

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Install a default `tracing` subscriber with INFO level unless overridden by
/// the `RUST_LOG` environment variable.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();
}
