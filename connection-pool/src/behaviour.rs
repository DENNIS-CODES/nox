/*
 * Copyright 2024 Fluence DAO
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use futures::{Sink, StreamExt};
use libp2p::core::Endpoint;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::CloseConnection::All;
use libp2p::swarm::{
    dial_opts, ConnectionDenied, ConnectionId, DialError, FromSwarm, ListenFailure, THandler,
    THandlerOutEvent, ToSwarm,
};
use libp2p::{
    core::{ConnectedPoint, Multiaddr},
    swarm::{NetworkBehaviour, NotifyHandler, OneShotHandler},
    PeerId,
};
use std::pin::Pin;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    task::{Context, Poll, Waker},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::PollSender;

use crate::connection_pool::LifecycleEvent;
use crate::{Command, ConnectionPoolApi};
use fluence_libp2p::remote_multiaddr;
use particle_protocol::{
    CompletionChannel, Contact, ExtendedParticle, HandlerMessage, ProtocolConfig, SendStatus,
};
use peer_metrics::ConnectionPoolMetrics;

// type SwarmEventType = generate_swarm_event_type!(ConnectionPoolBehaviour);

// TODO: replace with generate_swarm_event_type
type SwarmEventType = ToSwarm<(), HandlerMessage>;

#[derive(Debug, Default)]
/// [Peer] is the representation of [Contact] extended with precise connectivity information
struct Peer {
    /// Current peer has active connections with that list of addresses
    connected: HashSet<Multiaddr>,
    /// Addresses gathered via Identify protocol, but not connected
    discovered: HashSet<Multiaddr>,
    /// Dialed but not yet connected addresses
    dialing: HashSet<Multiaddr>,
    /// Channels to notify when any dial succeeds or peer is already connected
    dial_promises: Vec<oneshot::Sender<bool>>,
    // TODO: this layout of `dialing` and `dial_promises` doesn't allow to check specific addresses for reachability
    //       if check reachability for specific maddrs is ever required, one would need to maintain the following info:
    //       reachability_promises: HashMap<Multiaddr, Vec<oneshot::Sender<bool>>
}

impl Peer {
    pub fn addresses(&self) -> impl Iterator<Item = &Multiaddr> {
        self.connected
            .iter()
            .chain(&self.discovered)
            .chain(&self.dialing)
            .collect::<HashSet<_>>()
            .into_iter()
    }

    pub fn connected(addresses: impl IntoIterator<Item = Multiaddr>) -> Self {
        Peer {
            connected: addresses.into_iter().collect(),
            discovered: Default::default(),
            dialing: Default::default(),
            dial_promises: vec![],
        }
    }

    pub fn dialing(
        addresses: impl IntoIterator<Item = Multiaddr>,
        outlet: oneshot::Sender<bool>,
    ) -> Self {
        Peer {
            connected: Default::default(),
            discovered: Default::default(),
            dialing: addresses.into_iter().collect(),
            dial_promises: vec![outlet],
        }
    }
}

pub struct ConnectionPoolBehaviour {
    peer_id: PeerId,

    commands: UnboundedReceiverStream<Command>,

    outlet: PollSender<ExtendedParticle>,
    subscribers: Vec<mpsc::UnboundedSender<LifecycleEvent>>,

    queue: VecDeque<ExtendedParticle>,
    contacts: HashMap<PeerId, Peer>,
    dialing: HashMap<Multiaddr, Vec<oneshot::Sender<Option<Contact>>>>,

    events: VecDeque<SwarmEventType>,
    waker: Option<Waker>,
    pub(super) protocol_config: ProtocolConfig,

    metrics: Option<ConnectionPoolMetrics>,
}

impl ConnectionPoolBehaviour {
    fn execute(&mut self, cmd: Command) {
        match cmd {
            Command::Dial { addr, out } => self.dial(addr, out),
            Command::Connect { contact, out } => self.connect(contact, out),
            Command::Disconnect { peer_id, out } => self.disconnect(peer_id, out),
            Command::IsConnected { peer_id, out } => self.is_connected(peer_id, out),
            Command::GetContact { peer_id, out } => self.get_contact(peer_id, out),
            Command::Send { to, particle, out } => self.send(to, particle, out),
            Command::CountConnections { out } => self.count_connections(out),
            Command::LifecycleEvents { out } => self.add_subscriber(out),
        }
    }

    /// Dial `address`, and send contact back on success
    /// `None` means something prevented us from connecting - dial reach failure or something else
    pub fn dial(&mut self, address: Multiaddr, out: oneshot::Sender<Option<Contact>>) {
        // TODO: return Contact immediately if that address is already connected
        self.dialing.entry(address.clone()).or_default().push(out);

        self.push_event(ToSwarm::Dial {
            opts: DialOpts::unknown_peer_id().address(address).build(),
        });
    }

    /// Connect to the contact by all of its known addresses and return whether connection succeeded
    /// If contact is already being dialed and there are no new addresses in Contact, don't dial
    /// If contact is already connected, return `true` immediately
    pub fn connect(&mut self, new_contact: Contact, outlet: oneshot::Sender<bool>) {
        let addresses = match self.contacts.entry(new_contact.peer_id) {
            Entry::Occupied(mut entry) => {
                let known_contact = entry.get_mut();

                // collect previously unknown addresses
                let mut new_addrs = HashSet::new();
                // flag if `contact` has any unconnected addresses
                let mut not_connected = false;
                for maddr in new_contact.addresses {
                    if !known_contact.connected.contains(&maddr) {
                        not_connected = true;
                    }

                    if !known_contact.dialing.contains(&maddr) {
                        new_addrs.insert(maddr);
                    }
                }

                if not_connected {
                    // we got either new addresses to dial, or in-progress dialing on some
                    // addresses in `new_contact`, so remember to notify channel about dial state change
                    known_contact.dial_promises.push(outlet);
                } else {
                    // all addresses in `new_contact` are already connected, so notify about success
                    outlet.send(true).ok();
                }
                new_addrs.into_iter().collect()
            }
            Entry::Vacant(slot) => {
                slot.insert(Peer::dialing(new_contact.addresses.clone(), outlet));
                new_contact.addresses
            }
        };

        if !addresses.is_empty() {
            self.push_event(ToSwarm::Dial {
                opts: DialOpts::peer_id(new_contact.peer_id)
                    .addresses(addresses)
                    .build(),
            });
        }
    }

    pub fn disconnect(&mut self, peer_id: PeerId, outlet: oneshot::Sender<bool>) {
        self.push_event(ToSwarm::CloseConnection {
            peer_id,
            connection: All,
        });
        // TODO: signal disconnect completion only after `peer_removed` was called or Disconnect failed
        outlet.send(true).ok();
    }

    /// Returns whether given peer is connected or not
    pub fn is_connected(&self, peer_id: PeerId, outlet: oneshot::Sender<bool>) {
        outlet.send(self.contacts.contains_key(&peer_id)).ok();
    }

    /// Returns contact for a given peer if it is known
    pub fn get_contact(&self, peer_id: PeerId, outlet: oneshot::Sender<Option<Contact>>) {
        let contact = self.get_contact_impl(peer_id);
        outlet.send(contact).ok();
    }

    /// Sends a particle to a connected contact. Returns whether sending succeeded or not
    /// Result is sent to channel inside `upgrade_outbound` in ProtocolHandler
    pub fn send(
        &mut self,
        to: Contact,
        particle: ExtendedParticle,
        outlet: oneshot::Sender<SendStatus>,
    ) {
        let span =
            tracing::info_span!(parent: particle.span.as_ref(), "ConnectionPool::Behaviour::send");
        let _guard = span.enter();
        if to.peer_id == self.peer_id {
            // If particle is sent to the current node, process it locally
            self.queue.push_back(particle);
            outlet.send(SendStatus::Ok).ok();
            self.wake();
        } else if self.contacts.contains_key(&to.peer_id) {
            tracing::debug!(
                target: "network",
                particle_id = particle.particle.id ,
                "{}: Sending particle to {}",
                self.peer_id,
                to.peer_id
            );
            // Send particle to remote peer
            self.push_event(ToSwarm::NotifyHandler {
                peer_id: to.peer_id,
                handler: NotifyHandler::Any,
                event: HandlerMessage::OutParticle(
                    particle.particle,
                    CompletionChannel::Oneshot(outlet),
                ),
            });
        } else {
            tracing::warn!(
                particle_id = particle.particle.id,
                "Won't send particle to contact {}: not connected",
                to.peer_id
            );
            outlet.send(SendStatus::NotConnected).ok();
        }
    }

    /// Returns number of connected contacts
    pub fn count_connections(&mut self, outlet: oneshot::Sender<usize>) {
        outlet.send(self.contacts.len()).ok();
    }

    /// Subscribes given channel for all `LifecycleEvent`s
    pub fn add_subscriber(&mut self, outlet: mpsc::UnboundedSender<LifecycleEvent>) {
        self.subscribers.push(outlet);
    }

    pub fn add_discovered_addresses(&mut self, peer_id: PeerId, addresses: Vec<Multiaddr>) {
        self.contacts
            .entry(peer_id)
            .or_default()
            .discovered
            .extend(addresses);
    }

    fn meter<U, F: Fn(&ConnectionPoolMetrics) -> U>(&self, f: F) {
        self.metrics.as_ref().map(f);
    }
}

impl ConnectionPoolBehaviour {
    pub fn new(
        buffer: usize,
        protocol_config: ProtocolConfig,
        peer_id: PeerId,
        metrics: Option<ConnectionPoolMetrics>,
    ) -> (Self, mpsc::Receiver<ExtendedParticle>, ConnectionPoolApi) {
        let (outlet, inlet) = mpsc::channel(buffer);
        let outlet = PollSender::new(outlet);
        let (command_outlet, command_inlet) = mpsc::unbounded_channel();
        let api = ConnectionPoolApi {
            outlet: command_outlet,
            send_timeout: protocol_config.upgrade_timeout * 2,
        };

        let this = Self {
            peer_id,
            outlet,
            commands: UnboundedReceiverStream::new(command_inlet),
            subscribers: <_>::default(),
            queue: <_>::default(),
            contacts: <_>::default(),
            dialing: <_>::default(),
            events: <_>::default(),
            waker: None,
            protocol_config,
            metrics,
        };

        (this, inlet, api)
    }

    fn wake(&self) {
        if let Some(waker) = &self.waker {
            waker.wake_by_ref();
        }
    }

    fn add_connected_address(&mut self, peer_id: PeerId, maddr: Multiaddr) {
        // notify these waiting for a peer to be connected
        match self.contacts.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                let peer = entry.get_mut();
                peer.dialing.remove(&maddr);
                peer.discovered.remove(&maddr);
                peer.connected.insert(maddr.clone());

                let dial_promises = std::mem::take(&mut peer.dial_promises);

                for out in dial_promises {
                    out.send(true).ok();
                }
            }
            Entry::Vacant(e) => {
                e.insert(Peer::connected(std::iter::once(maddr.clone())));
            }
        }

        // notify these waiting for an address to be dialed
        if let Some(outs) = self.dialing.remove(&maddr) {
            let contact = self.get_contact_impl(peer_id);
            debug_assert!(contact.is_some());
            for out in outs {
                out.send(contact.clone()).ok();
            }
        }
        self.meter(|m| m.connected_peers.set(self.contacts.len() as i64));
    }

    fn lifecycle_event(&mut self, event: LifecycleEvent) {
        self.subscribers.retain(|out| {
            let ok = out.send(event.clone());
            ok.is_ok()
        })
    }

    fn push_event(&mut self, event: SwarmEventType) {
        self.events.push_back(event);
        self.wake();
    }

    fn remove_contact(&mut self, peer_id: &PeerId, reason: &str) {
        if let Some(contact) = self.contacts.remove(peer_id) {
            log::debug!("Contact {} was removed: {}", peer_id, reason);
            self.lifecycle_event(LifecycleEvent::Disconnected(Contact::new(
                *peer_id,
                contact.addresses().cloned().collect(),
            )));

            for out in contact.dial_promises {
                // if dial was in progress, notify waiters
                out.send(false).ok();
            }
            self.meter(|m| m.connected_peers.set(self.contacts.len() as i64));
        }
    }

    fn get_contact_impl(&self, peer_id: PeerId) -> Option<Contact> {
        self.contacts.get(&peer_id).map(|c| Contact {
            peer_id,
            addresses: c.addresses().cloned().collect(),
        })
    }

    fn on_connection_closed(
        &mut self,
        peer_id: &PeerId,
        cp: &ConnectedPoint,
        remaining_established: usize,
    ) {
        let multiaddr = remote_multiaddr(cp);
        if remaining_established == 0 {
            self.remove_contact(peer_id, "disconnected");
            log::debug!(
                target: "network",
                "{}: connection lost with {} @ {}",
                self.peer_id,
                peer_id,
                multiaddr
            );
        } else {
            log::debug!(
                target: "network",
                "{}: {} connections remaining established with {}. {} has just closed.",
                self.peer_id,
                remaining_established,
                peer_id,
                multiaddr
            )
        }

        self.cleanup_address(Some(peer_id), multiaddr);
    }

    fn on_dial_failure(&mut self, peer_id: Option<PeerId>, error: &DialError) {
        use dial_opts::PeerCondition::{Disconnected, NotDialing};
        if let DialError::DialPeerConditionFalse(Disconnected | NotDialing) = error {
            // So, if you tell libp2p to dial a peer, there's an option dial_opts::PeerCondition
            // The default one is Disconnected.
            // So, if you asked libp2p to connect to a peer, and the peer IS ALREADY CONNECTED,
            // libp2p will tell you that dial has failed.
            // We need to ignore this "failure" in case condition is Disconnected or NotDialing.
            // Because this basically means that peer has already connected while our Dial was processed.
            // That could happen in several cases:
            //  1. `dial` was called by multiaddress of an already-connected peer
            //  2. `connect` was called with new multiaddresses, but target peer is already connected
            //  3. unknown data race
            log::info!("Dialing attempt to an already connected peer {:?}", peer_id);
            return;
        }

        log::warn!(
            "Error dialing peer {}: {:?}",
            peer_id.map_or("unknown".to_string(), |id| id.to_string()),
            error
        );
        match error {
            DialError::WrongPeerId { endpoint, .. } => {
                let addr = match endpoint {
                    ConnectedPoint::Dialer { address, .. } => address,
                    ConnectedPoint::Listener { send_back_addr, .. } => send_back_addr,
                };
                self.cleanup_address(peer_id.as_ref(), addr);
            }
            DialError::Transport(addrs) => {
                for (addr, _) in addrs {
                    self.cleanup_address(peer_id.as_ref(), addr);
                }
            }
            _ => {}
        };
        // remove failed contact
        if let Some(peer_id) = peer_id {
            self.remove_contact(&peer_id, format!("dial failure: {error}").as_str())
        } else {
            log::warn!("Unknown peer dial failure: {}", error)
        }
    }

    fn on_listen_failure(&mut self, event: ListenFailure<'_>) {
        log::warn!(
            "Error accepting incoming connection from {} to our local address {}: {:?}",
            event.send_back_addr,
            event.local_addr,
            event.error
        );
    }

    fn cleanup_address(&mut self, peer_id: Option<&PeerId>, addr: &Multiaddr) {
        // Notify those who waits for address dial
        if let Some(outs) = self.dialing.remove(addr) {
            for out in outs {
                out.send(None).ok();
            }
        }

        let _: Option<()> = try {
            let peer_id = peer_id?;
            let contact = self.contacts.get_mut(peer_id)?;

            contact.connected.remove(addr);
            contact.discovered.remove(addr);
            contact.dialing.remove(addr);
            if contact.dialing.is_empty() {
                let dial_promises = std::mem::take(&mut contact.dial_promises);
                for out in dial_promises {
                    out.send(false).ok();
                }
            }
            if contact.connected.is_empty() && contact.dialing.is_empty() {
                self.remove_contact(
                    peer_id,
                    "no more connected or dialed addresses after 'cleanup_address' call",
                );
            }
        };
    }
}

impl NetworkBehaviour for ConnectionPoolBehaviour {
    type ConnectionHandler = OneShotHandler<ProtocolConfig, HandlerMessage, HandlerMessage>;
    type ToSwarm = ();

    fn handle_pending_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        Ok(())
    }

    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        peer_id: PeerId,
        _local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        log::debug!(
            target: "network",
            "{}: inbound connection established with {} @ {}",
            self.peer_id,
            peer_id,
            remote_addr
        );

        self.add_connected_address(peer_id, remote_addr.clone());

        self.lifecycle_event(LifecycleEvent::Connected(Contact::new(
            peer_id,
            vec![remote_addr.clone()],
        )));

        Ok(self.protocol_config.clone().into())
    }

    fn handle_pending_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        _addresses: &[Multiaddr],
        _effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        let peer_id = match maybe_peer {
            None => return Ok(vec![]),
            Some(peer_id) => peer_id,
        };
        Ok(self
            .contacts
            .get(&peer_id)
            .into_iter()
            .flat_map(|p| p.addresses().cloned())
            .collect())
    }

    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        peer_id: PeerId,
        addr: &Multiaddr,
        _role_override: Endpoint,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        log::debug!(
            target: "network",
            "{}: outbound connection established with {} @ {}",
            self.peer_id,
            peer_id,
            addr
        );

        self.add_connected_address(peer_id, addr.clone());

        self.lifecycle_event(LifecycleEvent::Connected(Contact::new(
            peer_id,
            vec![addr.clone()],
        )));
        Ok(self.protocol_config.clone().into())
    }

    fn on_swarm_event(&mut self, event: FromSwarm<'_>) {
        match event {
            FromSwarm::ConnectionEstablished(event) => {
                for addr in event.failed_addresses {
                    log::warn!("failed to connect to {} {}", addr, event.peer_id);
                    self.cleanup_address(Some(&event.peer_id), addr)
                }
            }
            FromSwarm::ConnectionClosed(event) => {
                self.on_connection_closed(
                    &event.peer_id,
                    event.endpoint,
                    event.remaining_established,
                );
            }
            FromSwarm::AddressChange(_) => {}
            FromSwarm::DialFailure(event) => {
                self.on_dial_failure(event.peer_id, event.error);
            }
            FromSwarm::ListenFailure(event) => {
                self.on_listen_failure(event);
            }
            FromSwarm::NewListener(_) => {}
            FromSwarm::NewListenAddr(_) => {}
            FromSwarm::ExpiredListenAddr(_) => {}
            FromSwarm::ListenerError(_) => {}
            FromSwarm::ListenerClosed(_) => {}

            FromSwarm::NewExternalAddrCandidate(_) => {}
            FromSwarm::ExternalAddrConfirmed(_) => {}
            FromSwarm::ExternalAddrExpired(_) => {}
            e => {
                tracing::warn!("Unexpected event {:?}", e);
                #[cfg(test)]
                panic!("Unexpected event")
            }
        }
    }

    fn on_connection_handler_event(
        &mut self,
        from: PeerId,
        _connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {
            Ok(HandlerMessage::InParticle(particle)) => {
                tracing::info!(target: "network", particle_id = particle.id,"{}: received particle from {}; queue {}", self.peer_id, from, self.queue.len());
                let root_span = tracing::info_span!("Particle", particle_id = particle.id);

                self.meter(|m| {
                    m.incoming_particle(
                        &particle.id,
                        self.queue.len() as i64 + 1,
                        particle.data.len() as f64,
                    )
                });
                self.queue
                    .push_back(ExtendedParticle::new(particle, root_span));
                self.wake();
            }
            Ok(HandlerMessage::Upgrade) => {}
            Ok(HandlerMessage::OutParticle(..)) => unreachable!("can't receive OutParticle"),
            Err(err) => log::warn!("Handler error: {:?}", err),
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<SwarmEventType> {
        self.waker = Some(cx.waker().clone());

        loop {
            // Check backpressure on the outlet
            let mut outlet = Pin::new(&mut self.outlet);
            match outlet.as_mut().poll_ready(cx) {
                Poll::Ready(Ok(_)) => {
                    // channel is ready to consume more particles, so send them
                    if let Some(particle) = self.queue.pop_front() {
                        let particle_id = particle.particle.id.clone();

                        if let Err(err) = outlet.start_send(particle) {
                            tracing::error!(
                                particle_id = particle_id,
                                "Failed to send particle to outlet: {}",
                                err
                            )
                        } else {
                            tracing::trace!(
                                target: "execution",
                                particle_id = particle_id,
                                "Sent particle to execution"
                            );
                        }
                    } else {
                        break;
                    }
                }
                Poll::Pending => {
                    // if channel is full, then keep particles in the queue
                    let len = self.queue.len();
                    if len > 30 {
                        log::warn!("Particle queue seems to have stalled; queue {}", len);
                    } else {
                        log::trace!(target: "network", "Connection pool outlet is pending; queue {}", len);
                    }
                    if self.outlet.is_closed() {
                        log::error!("Particle outlet closed");
                    }
                    break;
                }
                Poll::Ready(Err(err)) => {
                    log::warn!("ConnectionPool particle inlet has been dropped: {}", err);
                    break;
                }
            }
        }

        self.meter(|m| m.particle_queue_size.set(self.queue.len() as i64));
        while let Poll::Ready(Some(cmd)) = self.commands.poll_next_unpin(cx) {
            self.execute(cmd)
        }

        if let Some(event) = self.events.pop_front() {
            return Poll::Ready(event);
        }

        Poll::Pending
    }
}
