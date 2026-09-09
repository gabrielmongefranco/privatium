// Project:  Privatium™  |  File: crates/privatium-core/src/pair/join.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  A node joining a cluster as the client of /ws/pair (spec/protocol.md §2.3.1,
//           §7.4.2, spec/app-contract.md §6): the facts gathered under the node's lock
//           before the socket opens, the exchange driven on the socket with the lock
//           released, and what is applied under the lock once the direction is known —
//           adoption when this node joins, the row when it admits.
//           See main README.md for full license information.

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{SigningKey, VerifyingKey};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;
use zeroize::Zeroizing;

use super::handshake::{Client, ClientAdmitted, Direction, KIND_NODE, NodeClient, Paired};
use super::{Code, Joined, PairError};
use crate::identity::Certificate;
use crate::local::PeerHint;
use crate::{Error, Node, Result, log, new_ulid, sys};

/// How long the other node has to answer the socket at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The whole exchange, comfortably inside the thirty seconds the other node allows.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(45);

/// Frames on `/ws/pair` are a few kilobytes; the other node caps them the same way.
const MAX_FRAME: usize = 8192;

/// What a node's side of a join needs once the socket is open, gathered under the lock
/// by [`Node::join_start`] so nothing on the socket holds it. `Debug` prints no key.
pub struct JoinClient {
    client: Client,
    /// What this node's registry says about the node it dialed, for the case where this
    /// side admits (`§2.3.1`).
    registered: Registered,
    /// This node's cluster key, for the same case; wiped on drop.
    seed: Zeroizing<[u8; 32]>,
    /// This node's `_sys` counter at the start, sent in `admit` when this side admits.
    lam: u64,
    /// This node's display name, offered as the label of the row the other side writes.
    label: Option<String>,
    own_id: String,
    own_pub: VerifyingKey,
    url: String,
}

impl std::fmt::Debug for JoinClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinClient")
            .field("peer", &self.client.node_id())
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// The registry's word on the node dialed, read before the socket work (`§2.3.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Registered {
    Unknown,
    Readmit,
    Refused(RefusedWhy),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefusedWhy {
    Known,
}

/// What the exchange produced, to be applied under the lock by [`Node::join_apply`].
pub enum JoinOutcome {
    /// The other node admitted this one: adopt its cluster, then `joined` is sent.
    Adopt {
        /// What `admit` carried, verified.
        admitted: ClientAdmitted,
        /// The admitter, remembered as a peer hint (`spec/data-dictionary.md §3.7`).
        peer: PeerHint,
        /// Whether this node was re-admitted with its own key — it was expired at the
        /// exchange — in which case the certificate alone is replaced.
        readmission: bool,
    },
    /// This node admitted the other: write its row and `node.admitted`.
    Admitted {
        /// Origin dialed, retained as a local peer address (`spec/data-dictionary.md §3.7`).
        url: String,
        /// The other node's facts, as its device row carries them.
        paired: Paired,
        /// The `paired_at` sent in `admit`.
        paired_at: String,
        /// Whether this was a re-admission of a key already registered.
        readmission: bool,
    },
}

impl std::fmt::Debug for JoinOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Adopt { peer, .. } => f
                .debug_struct("Adopt")
                .field("peer", &peer.id)
                .finish_non_exhaustive(),
            Self::Admitted { paired, .. } => f
                .debug_struct("Admitted")
                .field("peer", &paired.device)
                .finish_non_exhaustive(),
        }
    }
}

/// The node-side steps of a join, so the socket driver can run with the node's lock
/// released between them: [`Node`] implements it directly for an embedder, and the
/// daemon's handler implements it by taking its lock for each call.
pub trait JoinNode {
    /// Read the other node's hello and produce this node's first message, gathering
    /// every fact the exchange needs from the node.
    fn join_start(&mut self, url: &str, code: Code, hello: &str) -> Result<(JoinClient, String)>;
    /// Apply the outcome: adopt the cluster, or write the admitted node's row.
    fn join_apply(&mut self, outcome: JoinOutcome, now: jiff::Timestamp) -> Result<Joined>;
}

/// `http://host[:port]`, and nothing else, as the URL of a node to join; the socket
/// path is added here. A scheme, path, query, fragment or user information anywhere is
/// refused, so the URL can go into no request but this one.
pub fn join_target(url: &str) -> Result<String> {
    let refuse = |problem: &str| Error::Join {
        url: url.to_owned(),
        problem: problem.to_owned(),
    };
    let rest = url
        .trim()
        .strip_prefix("http://")
        .ok_or_else(|| refuse("the URL must start with http:// (spec/cli.md §8)"))?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty()
        || authority.len() > 255
        || !authority
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
    {
        return Err(refuse(
            "the URL is a host and a port and nothing else, as the other node printed it",
        ));
    }
    let authority = if authority.ends_with(']') || !authority.contains(':') {
        format!("{authority}:8420")
    } else {
        authority.to_owned()
    };
    Ok(format!("ws://{authority}/ws/pair"))
}

impl Node {
    /// Join the cluster of the node at `url`, whose owner opened a window for a node and
    /// read out `code` (`spec/app-contract.md §6`, `spec/protocol.md §2.3.1`). Dials
    /// `<url>/ws/pair` on a runtime of its own and drives the exchange; whichever side
    /// `§2.3.1` admits, this node ends in the peer's cluster or the peer in this node's.
    /// Refused by name when the code is wrong, the other node refuses, or nobody can
    /// admit; the code never appears in the error.
    pub fn join(&mut self, url: &str, code: Code) -> Result<Joined> {
        let target = join_target(url)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| Error::Join {
                url: url.to_owned(),
                problem: format!("could not start a runtime: {error}"),
            })?;
        runtime.block_on(join_over_socket(&target, url, code, self))
    }

    /// The first node-side step of a join (`§2.3.1`): refuse a hello from this node
    /// itself or from a node this cluster revoked, then gather what the exchange needs —
    /// this node's keys, flags and display name, the registry's word on the peer, the
    /// cluster seed and the counter for the case where this side admits — and produce
    /// the first message.
    pub fn join_start_at(
        &mut self,
        url: &str,
        code: Code,
        hello: &str,
        now: jiff::Timestamp,
    ) -> Result<(JoinClient, String)> {
        let refuse = |problem: String| Error::Join {
            url: url.to_owned(),
            problem,
        };
        if self.standing(now)? == crate::Standing::Revoked {
            return Err(Error::NodeRevoked);
        }
        let flags = self.flags(now)?;
        let (client, text) = Client::start_node(
            hello,
            code,
            self.identity().signing_key(),
            self.identity().x25519_static(),
            flags,
        )
        .map_err(|error| match error {
            PairError::Closed => refuse(
                "the other node has no pairing window open for a node; run `privatium pair --node` there first".to_owned(),
            ),
            other => refuse(other.to_string()),
        })?;
        let peer = client.node_id().to_owned();
        if peer == self.id().as_str() {
            return Err(refuse("that URL is this node's own".to_owned()));
        }
        // A node this cluster revoked is refused before a byte is sent to it (§2.3.4).
        if self.node_revoked(&peer)? {
            return Err(Error::PeerRevoked { node: peer });
        }
        let (label, _) = self.node_row_public_facts()?;
        let registered = match self.registered_node(&peer, now)? {
            RegisteredNode::Unknown => Registered::Unknown,
            RegisteredNode::Readmit => Registered::Readmit,
            RegisteredNode::Refused => Registered::Refused(RefusedWhy::Known),
        };
        Ok((
            JoinClient {
                client,
                registered,
                seed: self.identity().cluster_seed(),
                lam: self.sys_log().lam(),
                label,
                own_id: self.id().as_str().to_owned(),
                own_pub: self.identity().verifying_key(),
                url: url.to_owned(),
            },
            text,
        ))
    }

    /// Apply what the exchange produced (`§2.3.1`): adopt the peer's cluster, or write
    /// the peer's `sys_device` row and `node.admitted` as the admitter does.
    pub fn join_apply_at(&mut self, outcome: JoinOutcome, now: jiff::Timestamp) -> Result<Joined> {
        match outcome {
            JoinOutcome::Adopt {
                admitted,
                peer,
                readmission,
            } => self.adopt_cluster(admitted, peer, readmission, now),
            JoinOutcome::Admitted {
                url,
                paired,
                paired_at,
                readmission,
            } => {
                let detail = serde_json::to_string(&serde_json::json!({
                    "source": paired.source.to_string(),
                    "dialed": true,
                    "readmitted": readmission,
                }))?;
                let audit_at = log::now();
                let row = sys::DeviceRow {
                    label: paired.label.as_deref(),
                    kind: KIND_NODE,
                    replica: true,
                    ed25519_pub: Some(&paired.ed25519_pub),
                    x25519_pub: Some(&paired.x25519_pub),
                    paired_at: Some(&paired_at),
                    paired_via: Some("lan"),
                    last_seen_at: None,
                    user_agent: None,
                    revoked_at: None,
                    revoked_reason: None,
                };
                self.sys_log_mut().batch(|batch| {
                    if !readmission {
                        batch.put(sys::DEVICE, &paired.device, &row)?;
                    }
                    batch.put(
                        sys::AUDIT,
                        &new_ulid(),
                        &sys::AuditRow::alert(
                            &audit_at,
                            sys::KIND_NODE_ADMITTED,
                            Some(&paired.device),
                            &detail,
                        ),
                    )
                })?;
                self.state.remember_peer(PeerHint {
                    id: paired.device.clone(),
                    x25519_pub: paired.x25519_pub.clone(),
                    url: Some(url),
                });
                self.state.flush()?;
                self.refresh()?;
                self.publish_facts()?;
                Ok(Joined {
                    cluster_id: self.identity().cluster_id().as_str().to_owned(),
                    peer: paired.device,
                    joined: false,
                    readmitted: readmission,
                })
            }
        }
    }
}

/// What the registry says about a node this one dialed, for the case where this side
/// admits — the same rule as the node side applies (`§2.3.1`).
pub(crate) enum RegisteredNode {
    Unknown,
    Readmit,
    Refused,
}

impl JoinNode for Node {
    fn join_start(&mut self, url: &str, code: Code, hello: &str) -> Result<(JoinClient, String)> {
        self.join_start_at(url, code, hello, jiff::Timestamp::now())
    }

    fn join_apply(&mut self, outcome: JoinOutcome, now: jiff::Timestamp) -> Result<Joined> {
        self.join_apply_at(outcome, now)
    }
}

/// The exchange on the client side, as data: what to send after each message received.
/// The socket driver below and the in-process tests both go through it.
impl JoinClient {
    /// Read the node's `{"pB","cB"}` and produce `{"cA"}`.
    pub fn reply(&mut self, text: &str) -> Result<String> {
        self.client.reply(text).map_err(|error| self.refuse(error))
    }

    /// Open the node's sealed message and produce this node's, with this node's display
    /// name as the label. What follows is decided by the direction.
    pub fn finish(
        self,
        ciphertext: &[u8],
        now: jiff::Timestamp,
    ) -> Result<(JoinExchange, Vec<u8>)> {
        let url = self.url.clone();
        let (node, bytes) = self
            .client
            .finish_node(ciphertext, self.label.as_deref(), now)
            .map_err(|error| Error::Join {
                url: url.clone(),
                problem: error.to_string(),
            })?;
        Ok((
            JoinExchange {
                node,
                registered: self.registered,
                seed: self.seed,
                lam: self.lam,
                own_id: self.own_id,
                own_pub: self.own_pub,
                url,
            },
            bytes,
        ))
    }

    fn refuse(&self, error: PairError) -> Error {
        Error::Join {
            url: self.url.clone(),
            problem: error.to_string(),
        }
    }
}

/// The client side after both sealed messages: the direction, and the two ways on.
pub struct JoinExchange {
    node: NodeClient,
    registered: Registered,
    seed: Zeroizing<[u8; 32]>,
    lam: u64,
    own_id: String,
    own_pub: VerifyingKey,
    url: String,
}

impl std::fmt::Debug for JoinExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinExchange")
            .field("peer", &self.node.paired.node_id)
            .finish_non_exhaustive()
    }
}

/// The next step of a join once the direction is known.
pub enum JoinStep {
    /// The other node admits: wait for its `admit`.
    AwaitAdmit(Box<JoinAwaiting>),
    /// This node admits: `admit` is sealed and sent, then its `joined` awaited.
    SendAdmit {
        /// The `admit` message to send.
        bytes: Vec<u8>,
        /// The state that opens `joined`.
        pending: Box<JoinAdmitting>,
    },
}

/// Waiting for the other node's `admit`.
pub struct JoinAwaiting {
    joiner: super::handshake::Joiner,
    own_id: String,
    own_pub: VerifyingKey,
    cluster_pub: VerifyingKey,
    peer: PeerHint,
    /// This node was expired at the exchange, so what follows is a re-admission.
    readmission: bool,
    url: String,
}

/// Waiting for the other node's `joined`, having sent `admit`.
pub struct JoinAdmitting {
    admitter: super::handshake::Admitter,
    paired: Paired,
    paired_at: String,
    readmission: bool,
    url: String,
}

impl JoinExchange {
    /// Decide the direction (`§2.3.1`) and, when this side admits, apply the registry's
    /// word and seal `admit`. Refused by name when nobody can admit or the other node's
    /// key is refused; nothing is sealed then.
    pub fn next(self, now: jiff::Timestamp) -> Result<JoinStep> {
        let refuse = |problem: String| Error::Join {
            url: self.url.clone(),
            problem,
        };
        let direction = self
            .node
            .direction()
            .map_err(|error| refuse(error.to_string()))?;
        let peer_id = self.node.paired.node_id.clone();
        let peer_x25519 = STANDARD.encode(self.node.paired.node_x25519.as_bytes());
        let peer_ed25519 = STANDARD.encode(self.node.paired.node_ed25519.as_bytes());
        let cluster_pub = self.node.paired.cluster_pub;
        let label = self.node.node_label.clone();
        let readmission = self.node.own.expired;
        match direction {
            Direction::NodeAdmits => Ok(JoinStep::AwaitAdmit(Box::new(JoinAwaiting {
                joiner: self.node.into_joiner(),
                own_id: self.own_id,
                own_pub: self.own_pub,
                cluster_pub,
                peer: PeerHint {
                    id: peer_id,
                    x25519_pub: peer_x25519,
                    url: Some(self.url.clone()),
                },
                readmission,
                url: self.url,
            }))),
            Direction::ClientAdmits => {
                let readmission = match self.registered {
                    Registered::Unknown => false,
                    Registered::Readmit => true,
                    Registered::Refused(RefusedWhy::Known) => {
                        return Err(refuse(PairError::DeviceKnown.to_string()));
                    }
                };
                let certificate =
                    Certificate::issue_with(&self.seed, &self.node.paired.node_ed25519, now)
                        .map_err(|error| refuse(error.to_string()))?;
                let paired_at = log::format_ts(now);
                let mut admitter = self.node.into_admitter();
                let bytes = admitter
                    .admit(&self.seed, &certificate, &paired_at, self.lam)
                    .map_err(|error| refuse(error.to_string()))?;
                Ok(JoinStep::SendAdmit {
                    bytes,
                    pending: Box::new(JoinAdmitting {
                        admitter,
                        paired: Paired {
                            device: peer_id,
                            kind: KIND_NODE.to_owned(),
                            ed25519_pub: peer_ed25519,
                            x25519_pub: peer_x25519,
                            label,
                            user_agent: None,
                            source: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                        },
                        paired_at,
                        readmission,
                        url: self.url,
                    }),
                })
            }
        }
    }
}

impl JoinAwaiting {
    /// Verify the other node's `admit` (`§7.4.2`): the certificate under the key it
    /// came with, that key against the cluster key message 5 named, and this node's own
    /// identity. What comes back is applied by [`Node::join_apply_at`] before `joined`
    /// is sealed.
    pub fn verify(&mut self, ciphertext: &[u8], now: jiff::Timestamp) -> Result<JoinOutcome> {
        let admitted = self
            .joiner
            .verify(
                ciphertext,
                &self.own_id,
                &self.own_pub,
                Some(&self.cluster_pub),
                now,
            )
            .map_err(|error| Error::Join {
                url: self.url.clone(),
                problem: error.to_string(),
            })?;
        Ok(JoinOutcome::Adopt {
            admitted,
            peer: self.peer.clone(),
            readmission: self.readmission,
        })
    }

    /// Seal `joined`, once the cluster has been adopted.
    pub fn joined(self) -> Result<Vec<u8>> {
        self.joiner.joined().map_err(|error| Error::Join {
            url: self.url,
            problem: error.to_string(),
        })
    }
}

impl JoinAdmitting {
    /// Open the other node's `joined`; what comes back is applied by
    /// [`Node::join_apply_at`].
    pub fn admitted(self, ciphertext: &[u8]) -> Result<JoinOutcome> {
        self.admitter
            .admitted(ciphertext)
            .map_err(|error| Error::Join {
                url: self.url.clone(),
                problem: error.to_string(),
            })?;
        Ok(JoinOutcome::Admitted {
            url: self.url,
            paired: self.paired,
            paired_at: self.paired_at,
            readmission: self.readmission,
        })
    }
}

/// Dial `target` and drive the exchange with `node`'s lock released between its steps.
pub(crate) async fn join_over_socket(
    target: &str,
    url: &str,
    code: Code,
    node: &mut impl JoinNode,
) -> Result<Joined> {
    let refuse = |problem: String| Error::Join {
        url: url.to_owned(),
        problem,
    };
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let connected = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async_with_config(target, Some(config), false),
    )
    .await
    .map_err(|_| {
        refuse("no answer from the other node; is it running and reachable at that URL?".to_owned())
    })?
    .map_err(|error| refuse(format!("could not open the pairing socket: {error}")))?;
    let (mut socket, _) = connected;
    let exchange = async {
        let hello = expect_text(&mut socket, url).await?;
        let (client, start) = node.join_start(url, code, &hello)?;
        send(&mut socket, Message::Text(start.into()), url).await?;
        let reply = expect_text(&mut socket, url).await?;
        let mut client = client;
        let confirm = client.reply(&reply)?;
        send(&mut socket, Message::Text(confirm.into()), url).await?;
        let sealed = expect_binary(&mut socket, url).await?;
        let (exchange, bytes) = client.finish(&sealed, jiff::Timestamp::now())?;
        send(&mut socket, Message::Binary(bytes.into()), url).await?;
        let joined = match exchange.next(jiff::Timestamp::now())? {
            JoinStep::AwaitAdmit(mut awaiting) => {
                let admit = expect_binary(&mut socket, url).await?;
                let outcome = awaiting.verify(&admit, jiff::Timestamp::now())?;
                let joined = node.join_apply(outcome, jiff::Timestamp::now())?;
                let bytes = awaiting.joined()?;
                send(&mut socket, Message::Binary(bytes.into()), url).await?;
                joined
            }
            JoinStep::SendAdmit { bytes, pending } => {
                send(&mut socket, Message::Binary(bytes.into()), url).await?;
                let joined_bytes = expect_binary(&mut socket, url).await?;
                let outcome = pending.admitted(&joined_bytes)?;
                node.join_apply(outcome, jiff::Timestamp::now())?
            }
        };
        let _ = socket.close(None).await;
        Ok(joined)
    };
    tokio::time::timeout(EXCHANGE_TIMEOUT, exchange)
        .await
        .map_err(|_| refuse("the other node did not finish the exchange in time".to_owned()))?
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(socket: &mut Socket, message: Message, url: &str) -> Result<()> {
    socket.send(message).await.map_err(|error| Error::Join {
        url: url.to_owned(),
        problem: format!("the connection was lost: {error}"),
    })
}

/// The next message, or the other node's refusal by name when it closed instead.
async fn next(socket: &mut Socket, url: &str) -> Result<Message> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(Message::Close(frame))) => {
                let reason = frame
                    .map(|f| f.reason.to_string())
                    .filter(|r| !r.is_empty())
                    .unwrap_or_else(|| "the other node closed the connection".to_owned());
                return Err(Error::Join {
                    url: url.to_owned(),
                    problem: format!("the other node refused: {reason}"),
                });
            }
            Some(Ok(message)) => return Ok(message),
            Some(Err(error)) => {
                return Err(Error::Join {
                    url: url.to_owned(),
                    problem: format!("the connection was lost: {error}"),
                });
            }
            None => {
                return Err(Error::Join {
                    url: url.to_owned(),
                    problem: "the other node closed the connection".to_owned(),
                });
            }
        }
    }
}

async fn expect_text(socket: &mut Socket, url: &str) -> Result<String> {
    match next(socket, url).await? {
        Message::Text(text) => Ok(text.to_string()),
        _ => Err(Error::Join {
            url: url.to_owned(),
            problem: "the other node sent an unexpected message".to_owned(),
        }),
    }
}

async fn expect_binary(socket: &mut Socket, url: &str) -> Result<Vec<u8>> {
    match next(socket, url).await? {
        Message::Binary(bytes) => Ok(bytes.to_vec()),
        _ => Err(Error::Join {
            url: url.to_owned(),
            problem: "the other node sent an unexpected message".to_owned(),
        }),
    }
}

/// A clone of this node's signing key for the client of a join. `SigningKey` wipes
/// itself on drop, and the clone lives as long as the exchange.
impl crate::identity::Identity {
    pub(crate) fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.signing_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_join_url_is_a_host_and_a_port_and_nothing_else() {
        assert_eq!(
            join_target("http://192.0.2.5:8420").ok().as_deref(),
            Some("ws://192.0.2.5:8420/ws/pair")
        );
        assert_eq!(
            join_target("http://192.0.2.5:8420/").ok().as_deref(),
            Some("ws://192.0.2.5:8420/ws/pair")
        );
        assert_eq!(
            join_target("http://desk.local").ok().as_deref(),
            Some("ws://desk.local:8420/ws/pair")
        );
        assert_eq!(
            join_target("http://[fd00::5]:8420").ok().as_deref(),
            Some("ws://[fd00::5]:8420/ws/pair")
        );
        for bad in [
            "https://192.0.2.5:8420",
            "192.0.2.5:8420",
            "http://",
            "http://192.0.2.5:8420/settings",
            "http://user@192.0.2.5:8420",
            "http://192.0.2.5:8420?x=1",
            "http://192.0.2.5:8420#f",
        ] {
            assert!(join_target(bad).is_err(), "{bad}");
        }
    }
}
