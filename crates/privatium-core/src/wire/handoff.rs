// Project:  Privatium™  |  File: crates/privatium-core/src/wire/handoff.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Bounded, single-consumer response ownership across browser navigation (§8.3.1).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::Response;
use crate::http::auth::Session;

const NODE_LIMIT: usize = 32;
const DEVICE_LIMIT: usize = 4;
const TTL: Duration = Duration::from_secs(120);

struct Entry {
    owner: Session,
    response: Option<Response>,
    expires: Option<Instant>,
    timer: Option<tokio::task::AbortHandle>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(timer) = &self.timer {
            timer.abort();
        }
    }
}

/// Shared transient responses. Bodies are not polled until one authenticated consumer
/// takes ownership; neither the response nor its reference is persisted (§8.3.1).
#[derive(Clone, Default)]
pub(super) struct Handoffs(Arc<Mutex<HashMap<String, Entry>>>);

impl Handoffs {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reserve capacity before dispatch. None means the handler must not be called.
    pub(super) fn reserve(&self, owner: &Session) -> Option<Reservation> {
        let mut entries = self.lock();
        entries.retain(|_, e| e.expires.is_none_or(|at| at > Instant::now()));
        if entries.len() >= NODE_LIMIT
            || entries
                .values()
                .filter(|e| e.owner.device == owner.device)
                .count()
                >= DEVICE_LIMIT
        {
            return None;
        }
        let id = crate::new_ulid();
        entries.insert(
            id.clone(),
            Entry {
                owner: owner.clone(),
                response: None,
                expires: None,
                timer: None,
            },
        );
        Some(Reservation {
            store: self.clone(),
            id: Some(id),
        })
    }

    /// Consume only a ready response owned by this still-authenticated session.
    /// The caller checks current revocation before calling; failed ownership leaves it intact.
    pub(super) fn take(&self, owner: &Session, id: &str) -> Option<Response> {
        if !ulid::Ulid::from_string(id).is_ok_and(|value| value.to_string() == id) {
            return None;
        }
        let mut entries = self.lock();
        let entry = entries.get(id)?;
        if entry.owner.device != owner.device
            || entry.owner.node != owner.node
            || entry.owner.x25519 != owner.x25519
        {
            return None;
        }
        if entry.expires.is_some_and(|at| at <= Instant::now()) {
            entries.remove(id);
            return None;
        }
        entry.response.as_ref()?;
        entries.remove(id)?.response.take()
    }
}

/// A slot that is removed on cancellation or handler failure before publication.
pub(super) struct Reservation {
    store: Handoffs,
    id: Option<String>,
}

impl Reservation {
    /// Publish an unpolled body and schedule its release independently of socket lifetime.
    pub(super) fn publish(mut self, response: Response) -> Result<String, ()> {
        let id = self.id.take().ok_or(())?;
        let at = Instant::now() + TTL;
        let mut entries = self.store.lock();
        let entry = entries.get_mut(&id).ok_or(())?;
        entry.response = Some(response);
        entry.expires = Some(at);
        let weak = Arc::downgrade(&self.store.0);
        let expires_id = id.clone();
        entry.timer = Some(
            tokio::spawn(async move {
                tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await;
                if let Some(store) = weak.upgrade() {
                    store
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&expires_id);
                }
            })
            .abort_handle(),
        );
        Ok(id)
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.store.lock().remove(&id);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::body::Body;
    use futures_util::stream;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn owner(byte: u8) -> Session {
        let key = ed25519_dalek::SigningKey::from_bytes(&[byte; 32]);
        let id = crate::NodeId::derive(&key.verifying_key());
        Session {
            device: id.clone(),
            node: id,
            x25519: "synthetic".into(),
        }
    }

    #[tokio::test]
    async fn test_spec_8_3_1_retained_stream_is_not_polled_and_expiry_drops_it() {
        let polls = Arc::new(AtomicUsize::new(0));
        let observed = polls.clone();
        let stream = stream::poll_fn(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
            std::task::Poll::Ready(Some(Ok::<_, std::convert::Infallible>(
                axum::body::Bytes::from_static(b"synthetic"),
            )))
        });
        let store = Handoffs::default();
        let session = owner(1);
        let id = store
            .reserve(&session)
            .unwrap()
            .publish(Response::new(Body::from_stream(stream)))
            .unwrap();
        assert_eq!(polls.load(Ordering::Relaxed), 0);
        assert_eq!(Arc::strong_count(&polls), 2);
        store.lock().get_mut(&id).unwrap().expires = Some(Instant::now());
        assert!(store.take(&session, &id).is_none());
        assert_eq!(Arc::strong_count(&polls), 1);
        assert!(store.lock().is_empty());
    }

    #[test]
    fn test_spec_8_3_1_reservations_bound_node_capacity_and_cancel_without_a_body() {
        let store = Handoffs::default();
        let mut slots = Vec::new();
        for device in 1..=8 {
            for _ in 0..4 {
                slots.push(store.reserve(&owner(device)).unwrap());
            }
        }
        assert!(store.reserve(&owner(9)).is_none());
        slots.pop();
        assert!(store.reserve(&owner(9)).is_some());
        drop(slots);
        assert!(store.lock().is_empty());
        assert!(store.take(&owner(1), "").is_none());
        assert!(store.take(&owner(1), "invalid").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn test_spec_8_3_1_idle_expiry_releases_body_without_another_request() {
        let store = Handoffs::default();
        let id = store
            .reserve(&owner(1))
            .unwrap()
            .publish(Response::new(Body::empty()))
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(TTL + Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(!store.lock().contains_key(&id));
    }
}
