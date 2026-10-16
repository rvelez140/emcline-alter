//! Tiempo real (SSE): una sola suscripción Redis por proceso (patrón
//! `rvmail:events:*`) que se reparte en memoria a cada conexión.

use super::auth::Session;
use super::state::Shared;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use futures::{Stream, StreamExt};
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

pub fn routes() -> Router<Shared> {
    Router::new().route("/events", get(stream))
}

/// Reenvía los eventos de Redis al canal en memoria; se reconecta si se cae.
pub async fn pump(client: redis::Client, tx: broadcast::Sender<(Uuid, String)>) {
    loop {
        match client.get_async_pubsub().await {
            Ok(mut ps) => {
                if ps.psubscribe("rvmail:events:*").await.is_ok() {
                    let mut msgs = ps.on_message();
                    while let Some(m) = msgs.next().await {
                        let channel = m.get_channel_name().to_string();
                        let Some(user) = channel
                            .strip_prefix("rvmail:events:")
                            .and_then(|u| Uuid::parse_str(u).ok())
                        else {
                            continue;
                        };
                        let payload: String = m.get_payload().unwrap_or_default();
                        let _ = tx.send((user, payload));
                    }
                }
            }
            Err(e) => tracing::warn!(err = %e, "suscripción de eventos en Redis falló"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn stream(
    State(st): State<Shared>,
    Session(u): Session,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = BroadcastStream::new(st.events.subscribe());
    let user = u.id;
    let events = rx.filter_map(move |m| async move {
        let (owner, payload) = m.ok()?;
        if owner != user {
            return None;
        }
        let ty = serde_json::from_str::<serde_json::Value>(&payload)
            .ok()?
            .get("type")?
            .as_str()?
            .to_string();
        Some(Ok(Event::default().event(ty).data(payload)))
    });
    // Ping cada 25 s para que los proxies no cierren la conexión
    Sse::new(events).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(25))
            .event(Event::default().event("ping").data("1")),
    )
}
