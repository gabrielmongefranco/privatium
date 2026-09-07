// Project:  Privatium™  |  File: crates/privatium-core/src/discover/mdns.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-06
// Summary:  DNS-SD over mDNS (spec/protocol.md §6.1): one registration of
//           `_privatium._tcp.local.` carrying the TXT record, replaced whenever the facts
//           change and when the pairing window closes; and a browser of the same type that
//           keeps every node it resolves keyed by `id`. The daemon runs its own thread; a
//           second thread here reads what it browses.
//           See main README.md for full license information.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};

use super::{Facts, Shared, txt};

/// The service type of `§6.1`.
pub const SERVICE_TYPE: &str = "_privatium._tcp.local.";

/// How often the browsing thread wakes to notice a stop or a pairing window that
/// expired without anyone telling the registration.
const TICK: Duration = Duration::from_millis(500);

/// What is currently registered, so an update can unregister a renamed instance and
/// the expiry check can see which `pair` value is on the wire.
struct Registered {
    fullname: String,
    pair: bool,
}

struct Advertiser {
    daemon: ServiceDaemon,
    registered: Mutex<Option<Registered>>,
}

impl Advertiser {
    /// Register the record for `facts` at `now`, replacing the previous registration —
    /// and unregistering it first when the instance name changed, since the daemon
    /// keys registrations by full name.
    fn register(&self, facts: &Facts, now: jiff::Timestamp) -> Result<(), String> {
        let instance = facts.instance_name();
        let fullname = format!("{instance}.{SERVICE_TYPE}");
        let pair = facts.pair(now);
        let properties: Vec<(String, String)> = txt::record(facts, now);
        let host = format!("{}.local.", facts.id);
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance,
            &host,
            "",
            facts.port,
            &properties[..],
        )
        .map_err(|error| format!("mDNS record: {error}"))?
        .enable_addr_auto();

        let mut registered = self
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = registered.as_ref()
            && previous.fullname != fullname
        {
            // Best effort: a name that is already gone is not a failure to advertise.
            let _ = self.daemon.unregister(&previous.fullname);
        }
        self.daemon
            .register(info)
            .map_err(|error| format!("mDNS register: {error}"))?;
        *registered = Some(Registered { fullname, pair });
        Ok(())
    }

    fn registered_pair(&self) -> Option<bool> {
        self.registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|r| r.pair)
    }
}

/// The running mDNS mechanism: advertiser and browser on one daemon.
pub struct Service {
    advertiser: Arc<Advertiser>,
    stop: Arc<AtomicBool>,
    browser: Option<JoinHandle<()>>,
}

impl Service {
    /// Start the daemon, register the record for the facts `shared` holds, and browse
    /// for every other node. The error is the platform's reason — no multicast
    /// interface, the socket refused — in one sentence.
    pub fn start(shared: Arc<Shared>) -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|error| format!("mDNS daemon: {error}"))?;
        let advertiser = Arc::new(Advertiser {
            daemon,
            registered: Mutex::new(None),
        });
        let facts = shared.facts();
        if let Err(why) = advertiser.register(&facts, jiff::Timestamp::now()) {
            let _ = advertiser.daemon.shutdown();
            return Err(why);
        }
        let events = advertiser
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|error| format!("mDNS browse: {error}"))?;

        let stop = Arc::new(AtomicBool::new(false));
        let browser = {
            let advertiser = Arc::clone(&advertiser);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("privatium-mdns".to_owned())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match events.recv_timeout(TICK) {
                            Ok(ServiceEvent::ServiceResolved(resolved)) => {
                                let now = jiff::Timestamp::now();
                                if let Some(found) = read_resolved(&resolved, now) {
                                    shared.absorb(found);
                                }
                            }
                            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                                shared.forget_instance(&fullname);
                            }
                            Ok(_) => {}
                            Err(mdns_sd::RecvTimeoutError::Timeout) => {}
                            Err(mdns_sd::RecvTimeoutError::Disconnected) => break,
                        }
                        // A window that expired flips `pair` on the wire without a call
                        // from the node (`§6.1`, `§7.5`).
                        let now = jiff::Timestamp::now();
                        let facts = shared.facts();
                        if advertiser
                            .registered_pair()
                            .is_some_and(|on| on != facts.pair(now))
                        {
                            let _ = advertiser.register(&facts, now);
                        }
                    }
                })
                .map_err(|error| format!("mDNS thread: {error}"))?
        };
        Ok(Self {
            advertiser,
            stop,
            browser: Some(browser),
        })
    }

    /// Replace the registration with one for `facts`. A refusal here is the daemon
    /// going away, which the drop of this service is about to do anyway.
    pub fn update(&self, facts: &Facts) {
        let _ = self.advertiser.register(facts, jiff::Timestamp::now());
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(registered) = self
            .advertiser
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            let _ = self.advertiser.daemon.unregister(&registered.fullname);
        }
        let _ = self.advertiser.daemon.shutdown();
        if let Some(browser) = self.browser.take() {
            let _ = browser.join();
        }
    }
}

/// A resolution as this module reads it: what [`txt::read`] needs and no more, so a test
/// can hand one in without a daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// The instance's full name, `<name>._privatium._tcp.local.`.
    pub fullname: String,
    /// Where it was resolved to.
    pub addrs: Vec<IpAddr>,
    /// The SRV port.
    pub port: u16,
    /// The TXT record.
    pub txt: BTreeMap<String, String>,
}

impl Seen {
    /// Into a [`super::Discovered`], or `None` for a record that is not `pv/1`'s.
    #[must_use]
    pub fn read(&self, now: jiff::Timestamp) -> Option<super::Discovered> {
        txt::read(
            &self.txt,
            self.addrs.clone(),
            self.port,
            &self.fullname,
            now,
        )
    }
}

fn read_resolved(resolved: &ResolvedService, now: jiff::Timestamp) -> Option<super::Discovered> {
    let seen = Seen {
        fullname: resolved.fullname.clone(),
        addrs: resolved
            .addresses
            .iter()
            .map(mdns_sd::ScopedIp::to_ip_addr)
            .collect(),
        port: resolved.port,
        txt: resolved
            .txt_properties
            .iter()
            .map(|property| (property.key().to_owned(), property.val_str().to_owned()))
            .collect(),
    };
    seen.read(now)
}
