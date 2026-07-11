use std::time::Duration;

use chrono::{DateTime, Utc};
use observa_shared::{Event, HealthStatus, HeartbeatEvent, InsightSnapshot, LogEvent, SecurityAlert, Severity};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::insight;
use crate::state::SharedState;

const LOOP_INTERVAL: Duration = Duration::from_secs(30);
const STAGGER_OFFSETS: [Duration; 6] = [
    Duration::from_secs(0),
    Duration::from_secs(7),
    Duration::from_secs(14),
    Duration::from_secs(21),
    Duration::from_secs(28),
    Duration::from_secs(35),
];
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(10);
const NOTIFICATION_SEVERITY_THRESHOLD: Severity = Severity::Error;

/// Spawn all background loops and return their handles.
pub fn spawn_background_tasks(
    state: SharedState,
    shutdown: watch::Receiver<bool>,
) -> Vec<JoinHandle<()>> {
    vec![
        spawn_heartbeat(state.clone(), shutdown.clone()),
        spawn_health_check(state.clone(), shutdown.clone()),
        spawn_alerting(state.clone(), shutdown.clone()),
        spawn_insight_digest(state.clone(), shutdown.clone()),
        spawn_maintenance(state.clone(), shutdown.clone()),
        spawn_notifications(state, shutdown),
    ]
}

fn spawn_maintenance(state: SharedState, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(LOOP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tokio::time::sleep(STAGGER_OFFSETS[4]).await;

        let mut last_vacuum = Utc::now();

        loop {
            ticker.tick().await;
            if *shutdown.borrow() {
                break;
            }

            if let Some(db) = state.db.as_ref() {
                let retention_days = state.config.retention_days;
                match db.prune_metrics(retention_days).await {
                    Ok(pruned) if pruned > 0 => {
                        info!(pruned, retention_days, "pruned old metrics");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(%error, "failed to prune metrics");
                    }
                }

                match db.prune_logs(retention_days).await {
                    Ok(pruned) if pruned > 0 => {
                        info!(pruned, retention_days, "pruned old logs");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(%error, "failed to prune logs");
                    }
                }

                let vacuum_interval = state.config.vacuum_interval_hours;
                if vacuum_interval > 0
                    && Utc::now().signed_duration_since(last_vacuum).num_hours() >= vacuum_interval as i64
                {
                    if let Err(error) = db.vacuum().await {
                        warn!(%error, "failed to vacuum database");
                    } else {
                        info!("vacuumed database");
                        last_vacuum = Utc::now();
                    }
                }
            }
        }
    })
}

fn spawn_heartbeat(state: SharedState, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(LOOP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tokio::time::sleep(STAGGER_OFFSETS[0]).await;

        loop {
            ticker.tick().await;
            if *shutdown.borrow() {
                break;
            }

            let seq = state.background.next_heartbeat_seq();
            let event = Event::Heartbeat(HeartbeatEvent { ts: Utc::now(), seq });
            if let Err(error) = state.bus.publish(event) {
                warn!(%error, "failed to publish heartbeat event");
            }
        }
    })
}

fn spawn_health_check(state: SharedState, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(LOOP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tokio::time::sleep(STAGGER_OFFSETS[1]).await;

        let base_url = state.config.llm_api_base.clone();
        let has_llm = state.llm.is_some();

        loop {
            ticker.tick().await;
            if *shutdown.borrow() {
                break;
            }

            let status = if !has_llm {
                HealthStatus::Healthy
            } else {
                check_llm_health(&base_url).await
            };

            state.background.set_health(status).await;
            debug!(?status, "background health check completed");
        }
    })
}

async fn check_llm_health(base_url: &str) -> HealthStatus {
    // Probe the OpenAI-compatible `/models` endpoint. Both Ollama and
    // llama.cpp expose this under the `/v1` root, so it is more portable than
    // the llama-server-specific `/health` URL.
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(HEALTH_TIMEOUT)
        .connect_timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(_) => return HealthStatus::Degraded,
    };

    match client.get(&url).send().await {
        Ok(response) if response.status().is_success() => HealthStatus::Healthy,
        Ok(response) => {
            warn!(status = %response.status(), "LLM /models endpoint returned non-2xx");
            HealthStatus::Degraded
        }
        Err(error) => {
            warn!(%error, "LLM /models health check failed");
            HealthStatus::Unhealthy
        }
    }
}

/// Fetch recent security alerts and the last alert timestamp the background
/// loops have already processed.  Callers filter and track `newest_ts`
/// according to their own semantics.
async fn fresh_security_alerts(state: &SharedState, limit: usize) -> (Vec<SecurityAlert>, DateTime<Utc>) {
    let alerts = state.store.security_alerts(limit).await.unwrap_or_default();
    let last_ts = state.background.last_alert_ts().await;
    (alerts, last_ts)
}

fn spawn_alerting(state: SharedState, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(LOOP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tokio::time::sleep(STAGGER_OFFSETS[2]).await;

        loop {
            ticker.tick().await;
            if *shutdown.borrow() {
                break;
            }

            let (alerts, last_ts) = fresh_security_alerts(&state, 50).await;
            if alerts.is_empty() {
                continue;
            }

            let mut newest_ts = last_ts;
            for alert in alerts {
                if alert.ts > last_ts {
                    if let Err(error) = state.bus.publish(Event::Alert(alert.clone())) {
                        warn!(%error, "failed to publish alert event");
                    } else {
                        debug!(ts = %alert.ts, "published alert event");
                    }
                    if alert.ts > newest_ts {
                        newest_ts = alert.ts;
                    }
                }
            }
            state.background.set_last_alert_ts(newest_ts).await;
        }
    })
}

#[derive(Debug, serde::Serialize)]
struct NotificationPayload {
    ts: String,
    severity: String,
    source: String,
    message: String,
    key: String,
}

fn spawn_notifications(state: SharedState, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(LOOP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tokio::time::sleep(STAGGER_OFFSETS[5]).await;

        let Some(webhook_url) = state.config.notifications_webhook_url.clone().filter(|s| !s.is_empty()) else {
            return;
        };
        if !state.config.notifications_enabled {
            return;
        }

        let client = match reqwest::Client::builder()
            .timeout(WEBHOOK_TIMEOUT)
            .connect_timeout(Duration::from_secs(3))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                warn!(%error, "failed to build webhook client");
                return;
            }
        };

        loop {
            ticker.tick().await;
            if *shutdown.borrow() {
                break;
            }

            let (alerts, last_ts) = fresh_security_alerts(&state, 50).await;
            if alerts.is_empty() {
                continue;
            }

            let mut newest_ts = last_ts;
            for alert in alerts {
                if alert.ts <= last_ts {
                    continue;
                }
                if alert.severity < NOTIFICATION_SEVERITY_THRESHOLD {
                    continue;
                }

                let payload = NotificationPayload {
                    ts: alert.ts.to_rfc3339(),
                    severity: format!("{:?}", alert.severity),
                    source: alert.source.clone(),
                    message: alert.message.clone(),
                    key: notification_key(&alert),
                };

                match client.post(&webhook_url).json(&payload).send().await {
                    Ok(response) if response.status().is_success() => {
                        debug!(key = %payload.key, "delivered webhook notification");
                    }
                    Ok(response) => {
                        warn!(status = %response.status(), key = %payload.key, "webhook notification returned non-2xx");
                    }
                    Err(error) => {
                        warn!(%error, key = %payload.key, "failed to deliver webhook notification");
                    }
                }

                if alert.ts > newest_ts {
                    newest_ts = alert.ts;
                }
            }

            if newest_ts > last_ts {
                state.background.set_last_alert_ts(newest_ts).await;
            }
        }
    })
}

fn notification_key(alert: &SecurityAlert) -> String {
    format!("{}:{}:{}:{}", alert.ts.timestamp(), alert.source, alert.severity as u8, alert.message)
}

fn spawn_insight_digest(state: SharedState, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(LOOP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tokio::time::sleep(STAGGER_OFFSETS[3]).await;

        loop {
            ticker.tick().await;
            if *shutdown.borrow() {
                break;
            }

            let metrics = state.store.recent_metrics(10).await.unwrap_or_default();
            let logs = state.store.recent_logs(20).await.unwrap_or_default();

            let summary = match state.llm_mode {
                crate::state::LlmMode::Fallback => insight::generate_local(&metrics, &logs),
                crate::state::LlmMode::Remote => match insight::generate(&state, &metrics, &logs).await {
                    Ok(text) => text,
                    Err(error) => {
                        warn!(%error, "insight digest generation failed");
                        continue;
                    }
                },
            };

            let health = insight::classify_health(&summary);
            let insight = InsightSnapshot {
                ts: Utc::now(),
                summary,
                health,
            };
            state.background.set_insight(insight.clone()).await;
            info!(summary = %insight.summary, "generated system insight");

            if health != HealthStatus::Healthy {
                let severity = if health == HealthStatus::Unhealthy {
                    Severity::Critical
                } else {
                    Severity::Warn
                };
                let alert_event = LogEvent {
                    ts: insight.ts,
                    source: "observa-insight".to_string(),
                    unit: None,
                    severity,
                    message: insight.summary.clone(),
                    raw: None,
                    security: true,
                };
                if let Some(db) = &state.db {
                    match observa_db::security::store(
                        db,
                        Uuid::new_v4(),
                        alert_event.ts,
                        &alert_event.source,
                        alert_event.unit.as_deref(),
                        alert_event.severity,
                        &alert_event.message,
                        &alert_event.raw,
                    )
                    .await
                    {
                        Ok(alert) => {
                            if let Err(error) = state.bus.publish(Event::Alert(alert)) {
                                warn!(%error, "failed to publish insight alert to event bus");
                            }
                        }
                        Err(error) => {
                            warn!(%error, "failed to persist insight security alert");
                        }
                    }
                }
            }
        }
    })
}


