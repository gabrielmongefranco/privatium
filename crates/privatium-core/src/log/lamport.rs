// Project:  Privatium™  |  File: crates/privatium-core/src/log/lamport.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-01
// Summary:  The Lamport counter of spec/protocol.md §4.3. Small enough to inline
//           everywhere, and a type rather than a bare u64 because getting its arithmetic
//           subtly wrong would corrupt §4.5 merge order silently.

/// One node's Lamport counter for one app (`spec/protocol.md §4.3`).
///
/// - On write: `lam = max(lam_local, lam_max_seen) + 1`.
/// - On receiving events: `lam_local = max(lam_local, max(received.lam))`.
///
/// [`observe`](Self::observe) folds the second rule in as events are seen, which reduces
/// the first to an increment. That is why [`tick`](Self::tick) looks too simple: the `max`
/// already happened.
///
/// `lam` establishes causal order and is the primary key of `§4.5`'s merge. `ts` is for
/// humans and for tie-breaking only — which is why a wrong clock cannot reorder history
/// but a wrong `lam` can.
///
/// The counter is per **app**, not per writer. In Phase 1 those coincide, because a node
/// has exactly one writer per app (`AGENTS.md` 2). They stop coinciding in Phase 3, when a
/// sync receiver writes another device's log (`§10.2`) and must fold those events into the
/// same counter without going through this node's writer — which is why the counter lives
/// on [`AppLog`](super::AppLog) and is passed into the writer rather than owned by it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Lamport(u64);

impl Lamport {
    /// A counter resuming from a known value. Zero on a log that has never been written.
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// Fold in the `lam` of an event this node has seen — read from any log file, or (from
    /// Phase 3) received from a peer.
    ///
    /// Monotonic by construction: a lower value cannot pull the counter back. That is what
    /// keeps the counter correct when a log file is replaced by an older copy, which a
    /// restore can produce and which a bare `= seen` would silently break.
    pub fn observe(&mut self, seen: u64) {
        self.0 = self.0.max(seen);
    }

    /// The `lam` for the next event this node writes, having advanced the counter.
    ///
    /// Saturating rather than wrapping. At one event per nanosecond a `u64` lasts about 584
    /// years, so this is unreachable in practice; wrapping to zero would reorder the entire
    /// log, and a stuck counter at least degrades in a way someone can notice.
    pub fn tick(&mut self) -> u64 {
        self.0 = self.0.saturating_add(1);
        self.0
    }

    /// The counter's current value — the `lam` of the last event written or seen.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_counter_starts_at_one() {
        let mut lamport = Lamport::default();
        assert_eq!(lamport.get(), 0);
        assert_eq!(lamport.tick(), 1);
        assert_eq!(lamport.tick(), 2);
    }

    /// `§4.3`'s `max`, in the direction that matters: seeing a higher value jumps the
    /// counter past it, so the next write is causally after everything seen.
    #[test]
    fn observing_a_higher_value_jumps_the_counter() {
        let mut lamport = Lamport::new(3);
        lamport.observe(8830);
        assert_eq!(lamport.tick(), 8831);
    }

    /// And in the direction that is easy to get wrong: seeing a *lower* value must not
    /// move the counter at all.
    #[test]
    fn observing_a_lower_value_does_nothing() {
        let mut lamport = Lamport::new(100);
        lamport.observe(4);
        assert_eq!(lamport.get(), 100);
        assert_eq!(lamport.tick(), 101);
    }

    #[test]
    fn the_counter_saturates_rather_than_wrapping() {
        let mut lamport = Lamport::new(u64::MAX);
        assert_eq!(lamport.tick(), u64::MAX);
    }
}
