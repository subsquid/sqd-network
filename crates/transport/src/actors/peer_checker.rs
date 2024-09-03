use std::{collections::HashMap, future::Future, net::IpAddr, sync::Arc, time::Duration};

use futures::{FutureExt, StreamExt};
use libp2p::{multiaddr::Protocol, swarm::NetworkBehaviour, Swarm};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{
    behaviour::{
        base,
        base::{BaseBehaviour, BaseBehaviourEvent, PeerProbed, TryProbeError},
        wrapped::TToSwarm,
    },
    util::{new_queue, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
    BehaviourWrapper, Multiaddr, PeerId, Wrapped,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PeerCheckerConfig {
    /// How many probes can be waiting in the queue (default: 1024)
    pub probe_queue_size: usize,
    /// Timeout for shutting down the transport (default: `DEFAULT_SHUTDOWN_TIMEOUT`)
    pub shutdown_timeout: Duration,
}

impl Default for PeerCheckerConfig {
    fn default() -> Self {
        Self {
            probe_queue_size: 1024,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeRequest {
    pub peer_id: PeerId,
    pub ip_addr: IpAddr,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeOk {
    dht_reachable: bool,
    listen_addrs: Vec<Multiaddr>,
    agent_version: Box<str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProbeResult {
    /// Probe was successfully resolved – established connection to peer
    Ok(ProbeOk),
    /// Probe failed – connection to peer cannot be established
    Failure { error: Box<str> },
    /// There is already an ongoing probe for this peer ID
    Ongoing,
    /// There are too many probes scheduled or ongoing now
    TooManyProbes,
    /// There was an internal error in the server
    ServerError,
}

impl ProbeResult {
    pub fn from_base(result: base::ProbeResult, dht_reachable: bool) -> Self {
        match result {
            base::ProbeResult::Timeout => Self::Failure {
                error: "Probe timed out".to_string().into_boxed_str(),
            },
            base::ProbeResult::Error(error) => Self::Failure { error },
            base::ProbeResult::Reachable {
                listen_addrs,
                agent_version,
            } => Self::Ok(ProbeOk {
                dht_reachable,
                listen_addrs,
                agent_version,
            }),
        }
    }
}

pub struct Probe {
    pub request: ProbeRequest,
    pub result_sender: oneshot::Sender<ProbeResult>,
}

pub struct PeerCheckerBehaviour {
    base: Wrapped<BaseBehaviour>,
    dht_probes: HashMap<PeerId, Probe>,
    direct_probes: HashMap<PeerId, Probe>,
}

impl PeerCheckerBehaviour {
    pub fn new(base: BaseBehaviour) -> Wrapped<Self> {
        Self {
            base: base.into(),
            dht_probes: Default::default(),
            direct_probes: Default::default(),
        }
        .into()
    }

    pub fn probe_peer(&mut self, probe: Probe) {
        // We start with DHT probe first, direct probe will be attempted if this fails
        self.probe_dht(probe)
    }

    fn probe_dht(&mut self, probe: Probe) {
        let peer_id = probe.request.peer_id;
        match self.base.try_probe_dht(peer_id) {
            Ok(_) => {
                self.dht_probes.insert(peer_id, probe);
            }
            Err(TryProbeError::Ongoing) => {
                _ = probe.result_sender.send(ProbeResult::Ongoing);
            }
            Err(TryProbeError::TooManyProbes) => {
                _ = probe.result_sender.send(ProbeResult::TooManyProbes);
            }
        }
    }

    fn probe_direct(&mut self, probe: Probe) {
        let peer_id = probe.request.peer_id;
        let addr = Multiaddr::from(probe.request.ip_addr)
            .with(Protocol::Udp(probe.request.port))
            .with(Protocol::Quic);
        match self.base.try_probe_direct(peer_id, addr) {
            Ok(_) => {
                self.direct_probes.insert(peer_id, probe);
            }
            Err(TryProbeError::Ongoing) => {
                _ = probe.result_sender.send(ProbeResult::Ongoing);
            }
            Err(TryProbeError::TooManyProbes) => {
                _ = probe.result_sender.send(ProbeResult::TooManyProbes);
            }
        }
    }

    // fn on_identify_event(&mut self, ev: identify::Event) -> Option<TToSwarm<Self>> {
    //     let (conn_id, peer_id, info) = match ev {
    //         identify::Event::Received {
    //             connection_id,
    //             peer_id,
    //             info,
    //         } => (connection_id, peer_id, info),
    //         _ => return None,
    //     };
    //     let identify::Info {
    //         agent_version,
    //         mut listen_addrs,
    //         ..
    //     } = info;
    //     // Filter out private addresses
    //     let listen_addrs = listen_addrs.into_iter().filter(addr_is_reachable).collect();
    //
    //     if let Some(probe) = self.remove_dht_probe(peer_id) {
    //         log::debug!("DHT probe for {peer_id} succeeded");
    //         let result = ProbeResult::Ok {
    //             dht_reachable: true,
    //             listen_addrs,
    //             agent_version: agent_version.into_boxed_str(),
    //         };
    //         _ = probe.result_sender.send(result);
    //         return None;
    //     }
    //
    //     let Some(probe) = self.remove_direct_probe(peer_id, conn_id) else {
    //         return None;
    //     };
    //     log::debug!("Direct probe for {peer_id} succeeded");
    //     let result = ProbeResult::Ok {
    //         dht_reachable: false,
    //         listen_addrs,
    //         agent_version: agent_version.into_boxed_str(),
    //     };
    //     _ = probe.result_sender.send(result);
    //     None
    // }
    //
    // fn on_dial_failure(&mut self, dial_failure: DialFailure) -> Option<TToSwarm<Self>> {
    //     let Some(peer_id) = dial_failure.peer_id else {
    //         return None;
    //     };
    //     let conn_id = dial_failure.connection_id;
    //     let Some(probe) = self.remove_direct_probe(peer_id, conn_id) else {
    //         return None;
    //     };
    //     let error = dial_failure.error.to_string().into_boxed_str();
    //     log::debug!("Direct probe for {peer_id} failed: {error}");
    //
    //     let result = ProbeResult::Failure { error };
    //     _ = probe.result_sender.send(result);
    //     None
    // }
    //
    // fn on_dht_timeout(&mut self, peer_id: PeerId) -> Option<TToSwarm<Self>> {
    //     let Some(probe) = self.remove_dht_probe(peer_id) else {
    //         log::warn!("Unknown DHT probe timeout peer_id={peer_id}");
    //         return None;
    //     };
    //     let addr = Multiaddr::from(probe.request.ip_addr)
    //         .with(Protocol::Udp(probe.request.port))
    //         .with(Protocol::Quic);
    //     log::debug!(
    //         "DHT probe for peer {peer_id} timed out. Scheduling direct probe on addr {addr}"
    //     );
    //     let dial_opts = DialOpts::peer_id(peer_id)
    //         .addresses(vec![addr])
    //         .condition(PeerCondition::Always)
    //         .build();
    //     let conn_id = dial_opts.connection_id();
    //     self.add_direct_probe(peer_id, conn_id, probe);
    //     Some(ToSwarm::Dial { opts: dial_opts })
    // }
    //
    // fn on_direct_timeout(
    //     &mut self,
    //     peer_id: PeerId,
    //     conn_id: ConnectionId,
    // ) -> Option<TToSwarm<Self>> {
    //     let Some(probe) = self.remove_direct_probe(peer_id, conn_id) else {
    //         log::warn!("Unknown direct probe timeout peer_id={peer_id} conn_id={conn_id}");
    //         return None;
    //     };
    //     log::debug!("Direct probe for peer {peer_id} timed out");
    //     let result = ProbeResult::Failure {
    //         error: "Timeout".to_string().into_boxed_str(),
    //     };
    //     _ = probe.result_sender.send(result);
    //     None
    // }

    fn on_peer_probed(&mut self, PeerProbed { peer_id, result }: PeerProbed) {
        if let Some(probe) = self.dht_probes.remove(&peer_id) {
            match result {
                base::ProbeResult::Reachable { .. } => {
                    _ = probe.result_sender.send(ProbeResult::from_base(result, true));
                }
                // If DHT probe failed, we schedule the direct probe
                _ => self.probe_direct(probe),
            }
        } else if let Some(probe) = self.direct_probes.remove(&peer_id) {
            _ = probe.result_sender.send(ProbeResult::from_base(result, false));
        } else {
            log::error!("Unknown probe. peer_id={peer_id} result={result:?}");
        }
    }
}

impl BehaviourWrapper for PeerCheckerBehaviour {
    type Inner = Wrapped<BaseBehaviour>;
    type Event = ();

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.base
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        match ev {
            BaseBehaviourEvent::PeerProbed(ev) => self.on_peer_probed(ev),
            _ => {}
        }
        None
    }
}

struct PeerCheckerTransport {
    swarm: Swarm<Wrapped<PeerCheckerBehaviour>>,
    probes_rx: crate::util::Receiver<Probe>,
}

impl PeerCheckerTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting peer checker P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = self.swarm.select_next_some() => {},
                Some(probe) = self.probes_rx.recv() => self.probe_peer(probe),
            }
        }
        log::info!("Shutting down peer checker P2P transport");
    }

    fn probe_peer(&mut self, probe: Probe) {
        self.swarm.behaviour_mut().probe_dht(probe);
    }
}

#[derive(Clone)]
pub struct PeerCheckerTransportHandle {
    probes_tx: crate::util::Sender<Probe>,
    _task_manager: Arc<TaskManager>,
}

impl PeerCheckerTransportHandle {
    fn new(
        probes_tx: crate::util::Sender<Probe>,
        transport: PeerCheckerTransport,
        shutdown_timeout: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            probes_tx,
            _task_manager: Arc::new(task_manager),
        }
    }

    pub fn probe_peer(&self, request: ProbeRequest) -> impl Future<Output = ProbeResult> {
        log::debug!("Queueing probe: {request:?}");
        let (result_sender, result_receiver) = oneshot::channel();
        let probe = Probe {
            request,
            result_sender,
        };
        match self.probes_tx.try_send(probe) {
            Ok(_) => result_receiver
                .map(|res| res.unwrap_or_else(|_| ProbeResult::ServerError))
                .boxed(),
            Err(_) => futures::future::ready(ProbeResult::TooManyProbes).boxed(),
        }
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<PeerCheckerBehaviour>>,
    config: PeerCheckerConfig,
) -> PeerCheckerTransportHandle {
    let (probes_tx, probes_rx) = new_queue(config.probe_queue_size, "probes");
    let transport = PeerCheckerTransport { swarm, probes_rx };
    PeerCheckerTransportHandle::new(probes_tx, transport, config.shutdown_timeout)
}
