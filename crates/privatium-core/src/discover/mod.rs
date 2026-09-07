// Project:  Privatium™  |  File: crates/privatium-core/src/discover/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-06
// Summary:  LAN discovery (spec/protocol.md §6): the facts a node advertises, the two
//           mechanisms that carry them — mDNS (§6.1) and the UDP responder (§6.4) — started
//           together and never in sequence (§6.5), and the nodes seen on the network keyed
//           by ID. Threads of its own, no async runtime, so an embedder without tokio can
//           call `Node::serve_discovery`. See main README.md for full license information.

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

pub mod mdns;
pub mod txt;
pub mod udp;

/// What a node says about itself (`spec/protocol.md §6.1`): the source of the TXT record,
/// the UDP answer and the manifest's `pair` flag, so the three never disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The Node ID — `id`, the key a client uses.
    pub id: String,
    /// The Cluster ID — `cl`.
    pub cluster: String,
    /// `nm`: `sys_node.display_name`, or the Node ID while none is set. Untruncated here;
    /// [`instance_name`](Self::instance_name) is the bounded form.
    pub name: String,
    /// `apps`: every mounted app's slug, sorted.
    pub apps: Vec<String>,
    /// The slugs eligible for a DNS-SD subtype: mounted, `nav.advertise = true`, and at
    /// most `MAX_ADVERTISED_SLUG` characters (`§6.1`).
    pub advertised: Vec<String>,
    /// `build`: `official`, `custom` or `fork:<name>`.
    pub build: String,
    /// When the open pairing window closes, or `None` while pairing is closed. Kept as an
    /// instant rather than a flag so an answer written after the window expires says
    /// `0` without anyone having told it so.
    pub pair_until: Option<jiff::Timestamp>,
    /// `p`: the HTTP port.
    pub port: u16,
}

/// The most bytes an mDNS instance name may hold (`§6.1`).
pub const INSTANCE_NAME_MAX: usize = 63;

/// The most nodes remembered from the network at once. A household has a handful; a
/// LAN full of strangers' nodes, or a flood of invented `id`s, stops here.
pub const FOUND_MAX: usize = 256;

impl Facts {
    /// The `pair` key at `now` (`§6.1`): `1` while a window is open.
    #[must_use]
    pub fn pair(&self, now: jiff::Timestamp) -> bool {
        self.pair_until.is_some_and(|until| now < until)
    }

    /// The instance name: the display name cut to 63 bytes at a character boundary and
    /// trimmed, or the Node ID when the name is empty (`§6.1`).
    #[must_use]
    pub fn instance_name(&self) -> String {
        let cut = bound_name(&self.name);
        if cut.is_empty() { self.id.clone() } else { cut }
    }

    /// The DNS-SD subtypes this node would advertise, one per eligible slug (`§6.1`).
    #[must_use]
    pub fn subtypes(&self) -> Vec<String> {
        self.advertised
            .iter()
            .map(|slug| format!("_{slug}._sub.{}", mdns::SERVICE_TYPE))
            .collect()
    }
}

/// A display name as an instance name may hold it (`§6.1`): trimmed, control characters
/// dropped, and cut to [`INSTANCE_NAME_MAX`] bytes at a character boundary. Applied to
/// this node's own name before it is advertised and to a name read off the network
/// before it is kept.
#[must_use]
pub fn bound_name(name: &str) -> String {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    let mut end = name.len().min(INSTANCE_NAME_MAX);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].trim_end().to_owned()
}

/// A node seen on the network, keyed by `id` and never by instance name (`§6.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The Node ID.
    pub id: String,
    /// The Cluster ID.
    pub cluster: String,
    /// The display name it advertised.
    pub name: String,
    /// The addresses it was seen at, sorted.
    pub addrs: Vec<IpAddr>,
    /// Its HTTP port.
    pub port: u16,
    /// The slugs it advertised, in its own order; a truncated list ends with `…`.
    pub apps: Vec<String>,
    /// Whether it reported pairing open.
    pub pair: bool,
    /// When this record was last refreshed.
    pub seen_at: jiff::Timestamp,
    /// The mDNS instance it was resolved from, so a removal can find it; empty for a
    /// UDP answer.
    pub instance: String,
}

/// How one mechanism fared at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Running.
    Started,
    /// Disabled by its `sys_setting`, or not asked for.
    Off,
    /// Asked for and refused by the platform; the reason, without secrets.
    Failed(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Started => f.write_str("started"),
            Self::Off => f.write_str("off"),
            Self::Failed(why) => write!(f, "not started: {why}"),
        }
    }
}

/// What [`Discovery::start`] did, per mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// mDNS advertisement and browsing (`§6.1`).
    pub mdns: Outcome,
    /// The UDP responder (`§6.4`).
    pub udp: Outcome,
}

impl Status {
    /// Whether anything at all is running.
    #[must_use]
    pub fn any_started(&self) -> bool {
        self.mdns == Outcome::Started || self.udp == Outcome::Started
    }
}

/// Whether a mechanism is wanted: the reading of its `sys_setting`
/// (`spec/data-dictionary.md §3.6`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Switch {
    /// Start it.
    On,
    /// The owner turned it off.
    Off,
    /// The setting could not be read as `true` or `false`; the mechanism stays off with
    /// this reason, since a value that cannot be read is not permission to start.
    Refused(String),
}

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Advertise and browse over mDNS.
    pub mdns: Switch,
    /// Answer UDP probes.
    pub udp: Switch,
    /// The responder's port — [`udp::PORT`] for a node, `0` for a test that wants any
    /// free port.
    pub udp_port: u16,
}

/// What the mechanisms share with the node: the current facts, read by every answer, and
/// the nodes seen so far.
pub struct Shared {
    facts: RwLock<Facts>,
    found: Mutex<BTreeMap<String, Discovered>>,
}

impl Shared {
    fn new(facts: Facts) -> Arc<Self> {
        Arc::new(Self {
            facts: RwLock::new(facts),
            found: Mutex::new(BTreeMap::new()),
        })
    }

    /// The facts as they stand.
    #[must_use]
    pub fn facts(&self) -> Facts {
        self.facts
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_facts(&self, facts: Facts) {
        *self.facts.write().unwrap_or_else(PoisonError::into_inner) = facts;
    }

    /// Record a node seen by any mechanism, replacing what was known under its `id`.
    /// The records come off the network, so the table is bounded: past
    /// [`FOUND_MAX`] the entry seen longest ago makes room.
    pub fn absorb(&self, discovered: Discovered) {
        let mut found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        if !found.contains_key(&discovered.id) && found.len() >= FOUND_MAX {
            let oldest = found
                .iter()
                .min_by_key(|(_, seen)| seen.seen_at)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                found.remove(&oldest);
            }
        }
        found.insert(discovered.id.clone(), discovered);
    }

    /// Forget the node resolved from `instance`.
    pub fn forget_instance(&self, instance: &str) {
        self.found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, seen| seen.instance != instance);
    }

    /// Every node seen, by ID.
    #[must_use]
    pub fn discovered(&self) -> Vec<Discovered> {
        self.found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }
}

/// The running mechanisms. Dropping it stops both.
pub struct Discovery {
    shared: Arc<Shared>,
    mdns: Option<mdns::Service>,
    udp: Option<udp::Responder>,
    status: Status,
}

impl Discovery {
    /// Start every mechanism `options` asks for, at once (`§6.5`). A mechanism the
    /// platform refuses — no multicast interface, the UDP port taken — is an
    /// [`Outcome::Failed`] in the status and never a reason for the other not to run,
    /// which is why this does not return `Result`.
    #[must_use]
    pub fn start(facts: Facts, options: Options) -> Self {
        let shared = Shared::new(facts);
        let (mdns, mdns_outcome) = match options.mdns {
            Switch::On => match mdns::Service::start(Arc::clone(&shared)) {
                Ok(service) => (Some(service), Outcome::Started),
                Err(why) => (None, Outcome::Failed(why)),
            },
            Switch::Off => (None, Outcome::Off),
            Switch::Refused(why) => (None, Outcome::Failed(why)),
        };
        let (udp, udp_outcome) = match options.udp {
            Switch::On => match udp::Responder::bind(options.udp_port, Arc::clone(&shared)) {
                Ok(responder) => (Some(responder), Outcome::Started),
                Err(error) => (None, Outcome::Failed(error.to_string())),
            },
            Switch::Off => (None, Outcome::Off),
            Switch::Refused(why) => (None, Outcome::Failed(why)),
        };
        Self {
            shared,
            mdns,
            udp,
            status: Status {
                mdns: mdns_outcome,
                udp: udp_outcome,
            },
        }
    }

    /// What started.
    #[must_use]
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// The port the UDP responder answers on, while it runs.
    #[must_use]
    pub fn udp_port(&self) -> Option<u16> {
        self.udp.as_ref().map(udp::Responder::port)
    }

    /// New facts: every later UDP answer carries them, and the mDNS registration is
    /// replaced (`§6.1`).
    pub fn update(&self, facts: Facts) {
        self.shared.set_facts(facts.clone());
        if let Some(service) = &self.mdns {
            service.update(&facts);
        }
    }

    /// The facts as the mechanisms currently answer with.
    #[must_use]
    pub fn facts(&self) -> Facts {
        self.shared.facts()
    }

    /// Every node seen so far, by ID (`§6.1`).
    #[must_use]
    pub fn discovered(&self) -> Vec<Discovered> {
        self.shared.discovered()
    }

    /// Record a node learned some other way — a UDP answer, a test's fixture — under
    /// its `id`, exactly as the browser records what it resolves.
    pub fn absorb(&self, seen: Discovered) {
        self.shared.absorb(seen);
    }

    /// Forget every node resolved from the mDNS instance `instance`.
    pub fn forget_instance(&self, instance: &str) {
        self.shared.forget_instance(instance);
    }
}

impl fmt::Debug for Discovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Discovery")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
