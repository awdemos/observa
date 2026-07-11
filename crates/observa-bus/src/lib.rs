use observa_shared::{Event, ObservaError, Result};
use tokio::sync::broadcast::{self, Receiver, Sender};

const DEFAULT_CAPACITY: usize = 256;

/// An in-memory broadcast bus for real-time `Event` distribution.
///
/// Subscribers receive every event published after they subscribe. Slow
/// subscribers may be dropped by `tokio::sync::broadcast` when they lag behind
/// the configured capacity; this is considered back-pressure, not a failure.
#[derive(Debug)]
pub struct Bus {
    sender: Sender<Event>,
    _receiver: Receiver<Event>,
}

impl Clone for Bus {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            _receiver: self.sender.subscribe(),
        }
    }
}

impl Bus {
    /// Create a new bus with the default channel capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a new bus with a custom channel capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, receiver) = broadcast::channel(capacity);
        Self {
            sender,
            _receiver: receiver,
        }
    }

    /// Subscribe to future events.
    pub fn subscribe(&self) -> Receiver<Event> {
        self.sender.subscribe()
    }

    /// Publish an event to all active subscribers.
    ///
    /// Returns the number of subscribers that received the event.
    pub fn publish(&self, event: Event) -> Result<usize> {
        self.sender
            .send(event)
            .map_err(|e| ObservaError::EventBus(e.to_string()))
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "axum")]
const fn event_name(event: &Event) -> &'static str {
    match event {
        Event::Metric(_) => "metric",
        Event::Log(_) => "log",
        Event::Chat(_) => "chat",
        Event::Heartbeat(_) => "heartbeat",
        Event::Alert(_) => "alert",
    }
}

/// Build an async stream of `Event`s from a bus subscription.
///
/// Lagged messages are silently skipped (back-pressure). The stream ends when
/// the bus is dropped.
pub fn event_stream(bus: &Bus) -> impl tokio_stream::Stream<Item = Event> {
    use tokio_stream::{wrappers::BroadcastStream, StreamExt};

    BroadcastStream::new(bus.subscribe()).filter_map(|res| match res {
        Ok(event) => Some(event),
        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(skipped)) => {
            tracing::debug!(skipped = skipped, "bus subscriber lagged; skipping events");
            None
        }
    })
}

#[cfg(feature = "axum")]
/// Build an SSE stream from a bus subscription.
///
/// Each `Event` is serialized to JSON and emitted as an SSE message whose
/// event name is `metric`, `log`, `chat`, `heartbeat`, or `alert`. The stream
/// silently skips lagged messages (back-pressure) and ends when the bus is
/// dropped.
pub fn sse_stream(
    bus: &Bus,
) -> axum::response::sse::Sse<
    impl tokio_stream::Stream<
            Item = std::result::Result<axum::response::sse::Event, std::convert::Infallible>,
        > + Send,
> {
    use tokio_stream::StreamExt;

    let stream = event_stream(bus).filter_map(|event| {
        let data = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(e) => {
                tracing::error!("failed to serialize event for sse: {e}");
                return None;
            }
        };

        Some(Ok(axum::response::sse::Event::default()
            .event(event_name(&event))
            .data(data)))
    });

    axum::response::sse::Sse::new(stream)
}
