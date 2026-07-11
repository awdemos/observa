use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use observa_bus::Bus;
use observa_cache::Cache;
use observa_db::Db;
use observa_shared::{Event, LogEvent, LogSource, Result};
use tokio::fs::File;
use uuid::Uuid;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{error, info, warn};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// Runtime options for the ingestor task.
pub struct IngestorOpts {
    pub source: LogSource,
    pub tail: bool,
    pub db: Option<Db>,
    pub cache: Option<Cache>,
    pub bus: Bus,
    pub shutdown: watch::Receiver<bool>,
}

/// Spawn an asynchronous log ingestor.
///
/// The ingestor runs `journalctl -o json -n 0 -f` when `LogSource::Journald` is
/// selected, bounded by a short timeout so a missing journalctl binary does not
/// block shutdown.  When `LogSource::File` is selected, it reads the file and
/// optionally tails it for new lines.
pub fn spawn_ingestor(opts: IngestorOpts) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        info!(?opts.source, "log ingestor started");

        let source = opts.source.clone();
        match source {
            LogSource::Journald => run_journalctl(opts).await,
            LogSource::File { path } => run_file(path, opts).await,
        };

        info!("log ingestor stopped");
        Ok(())
    })
}

async fn run_journalctl(opts: IngestorOpts) {
    info!("starting journalctl reader");

    let mut child = match Command::new("journalctl")
        .args(["-o", "json", "-n", "0", "-f"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            warn!(%error, "journalctl unavailable; no logs will be ingested");
            return;
        }
    };

    let stdout = child.stdout.take().expect("stdout was piped");
    let mut lines = BufReader::new(stdout).lines();

    loop {
        if *opts.shutdown.borrow() {
            break;
        }

        let line = match timeout(COMMAND_TIMEOUT, lines.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => {
                warn!("journalctl stdout closed");
                break;
            }
            Ok(Err(error)) => {
                warn!(%error, "journalctl read error");
                break;
            }
            Err(_) => {
                // Timeout: loop again and check shutdown; this prevents blocking
                // forever if journalctl produces no output.
                continue;
            }
        };

        match crate::parser::parse_journalctl_json(&line) {
            Ok(Some(event)) => {
                if let Err(error) = dispatch(event, &opts).await {
                    error!(%error, "failed to dispatch journald log event");
                }
            }
            Ok(None) => {}
            Err(error) => {
                warn!(%error, line = %line, "failed to parse journalctl line");
            }
        }
    }

    let _ = child.start_kill();
}

async fn run_file(path: PathBuf, opts: IngestorOpts) {
    info!(path = %path.display(), tail = opts.tail, "starting file log reader");

    let file = match File::open(&path).await {
        Ok(file) => file,
        Err(error) => {
            warn!(path = %path.display(), %error, "could not open log file; no logs will be ingested");
            return;
        }
    };

    let mut lines = BufReader::new(file).lines();

    loop {
        if *opts.shutdown.borrow() {
            break;
        }

        match timeout(Duration::from_millis(500), lines.next_line()).await {
            Ok(Ok(Some(line))) => {
                match crate::parser::parse_fallback_line(&line) {
                    Ok(event) => {
                        if let Err(error) = dispatch(event, &opts).await {
                            error!(%error, "failed to dispatch file log event");
                        }
                    }
                    Err(error) => warn!(%error, "failed to parse fallback log line"),
                }
            }
            Ok(Ok(None)) => {
                if opts.tail {
                    // Wait a bit for new lines; also respects shutdown via the
                    // outer loop timeout.
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                break;
            }
            Ok(Err(error)) => {
                warn!(path = %path.display(), %error, "log file read error");
                break;
            }
            Err(_) => continue,
        }
    }
}

async fn dispatch(event: LogEvent, opts: &IngestorOpts) -> Result<()> {
    if let Some(db) = &opts.db {
        db.store_log(&event).await?;
        if event.security {
            observa_db::security::store(
                db,
                Uuid::new_v4(),
                event.ts,
                &event.source,
                event.unit.as_deref(),
                event.severity,
                &event.message,
                &event.raw,
            )
            .await?;
        }
    }

    if let Some(cache) = &opts.cache {
        cache.push_recent_log(&event).await?;
    }

    opts.bus.publish(Event::Log(event))?;
    Ok(())
}
