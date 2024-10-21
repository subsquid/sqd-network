use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::{Debug, Display, Formatter},
    task::{Context, Poll},
    time::Duration,
    vec,
};

use derivative::Derivative;
use futures_bounded::FuturesMap;
use libp2p::{
    request_response,
    request_response::{Codec, OutboundFailure, OutboundRequestId, ProtocolSupport},
    swarm::{behaviour::ConnectionEstablished, FromSwarm, ToSwarm},
};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DurationSeconds};

use crate::{
    behaviour::wrapped::{BehaviourWrapper, TToSwarm},
    PeerId, QueueFull,
};

#[derive(Derivative)]
#[derivative(Debug, Clone)]
pub enum ClientEvent<T> {
    Response {
        peer_id: PeerId,
        req_id: OutboundRequestId,
        #[derivative(Debug = "ignore")]
        response: T,
    },
    PeerUnknown {
        peer_id: PeerId,
    },
    Timeout {
        peer_id: PeerId,
        req_id: OutboundRequestId,
        timeout: Timeout,
    },
    Failure {
        peer_id: PeerId,
        req_id: OutboundRequestId,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Timeout {
    /// Peer lookup or connection establishing timed out
    Lookup,
    /// Request was sent to worker, but waiting for response timed out
    Request,
}

impl Display for Timeout {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lookup => write!(f, "lookup timeout"),
            Self::Request => write!(f, "request timeout"),
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    /// Maximum number of buffered requests (default: 1024).
    pub max_buffered: usize,
    /// How long to wait for peer lookup and connection to be established (default: 10 sec).
    #[serde_as(as = "DurationSeconds")]
    #[serde(rename = "lookup_timeout_sec")]
    pub lookup_timeout: Duration,
    /// How long to wait for response, once the request has been sent (default: 60 sec).
    #[serde_as(as = "DurationSeconds")]
    #[serde(rename = "request_timeout_sec")]
    pub request_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_buffered: 1024,
            lookup_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
        }
    }
}

pub struct ClientBehaviour<C>
where
    C: Codec + Clone + Send + 'static,
    C::Request: Clone,
{
    inner: request_response::Behaviour<C>,
    /// Requests that were submitted for the first time (req_id -> request)
    original_requests: BTreeMap<OutboundRequestId, C::Request>,
    /// Requests that failed and wait for peer to be connected
    waiting_for_connection: HashMap<PeerId, HashSet<OutboundRequestId>>,
    /// Requests that were submitted for the second time, after the peer had been found (new_id -> old_id)
    resubmitted_requests: BTreeMap<OutboundRequestId, OutboundRequestId>,
    /// Timeouts for peer lookups
    lookup_timeouts: FuturesMap<PeerId, ()>,
    /// Maximum number of buffered requests
    max_buffered: usize,
}

impl<C> ClientBehaviour<C>
where
    C: Codec + Clone + Send + 'static,
    C::Request: Clone,
{
    pub fn new(
        codec: C,
        protocol: C::Protocol,
        ClientConfig {
            max_buffered,
            lookup_timeout,
            request_timeout,
        }: ClientConfig,
    ) -> Self {
        let inner = request_response::Behaviour::with_codec(
            codec,
            vec![(protocol, ProtocolSupport::Outbound)],
            request_response::Config::default().with_request_timeout(request_timeout),
        );
        Self {
            inner,
            original_requests: Default::default(),
            waiting_for_connection: Default::default(),
            resubmitted_requests: Default::default(),
            lookup_timeouts: FuturesMap::new(lookup_timeout, max_buffered),
            max_buffered,
        }
    }

    /// Try to send a request. It will be dropped if the outbound buffer is full
    pub fn try_send_request(
        &mut self,
        peer_id: PeerId,
        request: C::Request,
    ) -> Result<OutboundRequestId, QueueFull> {
        if self.original_requests.len() >= self.max_buffered {
            log::warn!("Outbound buffer full. Dropped message to {peer_id}");
            return Err(QueueFull);
        }

        let req_id = self.inner.send_request(&peer_id, request.clone());
        log::debug!("Sending request {req_id} to {peer_id}");

        // Buffer request for possible future retry
        self.original_requests.insert(req_id, request);

        Ok(req_id)
    }

    fn on_lookup_timeout(&mut self, peer_id: PeerId) -> Vec<TToSwarm<Self>> {
        let buffered = self.waiting_for_connection.remove(&peer_id).unwrap_or_default();
        log::debug!("Lookup for peer {peer_id} timed out, dropping {} requests", buffered.len());
        buffered
            .into_iter()
            .map(|req_id| {
                self.original_requests.remove(&req_id);
                ToSwarm::GenerateEvent(ClientEvent::Timeout {
                    peer_id,
                    req_id,
                    timeout: Timeout::Lookup,
                })
            })
            .collect()
    }

    fn on_connection_established(&mut self, peer_id: PeerId) {
        self.lookup_timeouts.remove(peer_id);
        let buffered = self.waiting_for_connection.remove(&peer_id).unwrap_or_default();
        log::debug!("Peer {peer_id} connected, sending {} requests", buffered.len());
        for old_id in buffered {
            let Some(request) = self.original_requests.remove(&old_id) else {
                log::error!("Unknown request: {old_id}");
                continue;
            };
            // Resubmit, keep the old ID in map to match the response with request
            let new_id = self.inner.send_request(&peer_id, request);
            log::debug!("Resubmitting request {old_id} as {new_id}");
            self.resubmitted_requests.insert(new_id, old_id);
        }
    }

    fn on_success(
        &mut self,
        peer_id: PeerId,
        req_id: OutboundRequestId,
        response: C::Response,
    ) -> Option<TToSwarm<Self>> {
        log::debug!("Request {req_id} successful");
        self.original_requests.remove(&req_id);
        let req_id = self.resubmitted_requests.remove(&req_id).unwrap_or(req_id);
        Some(ToSwarm::GenerateEvent(ClientEvent::Response {
            peer_id,
            req_id,
            response,
        }))
    }

    fn on_failure(
        &mut self,
        peer_id: PeerId,
        mut req_id: OutboundRequestId,
        error: OutboundFailure,
    ) -> Option<TToSwarm<Self>> {
        log::debug!("Request {req_id} failed: {error}");

        // If request was submitted for the first time and dial failed, try to find peer and connect
        // Keep the request contents in buffer for re-submitting, if lookup is successful
        if matches!(&error, OutboundFailure::DialFailure)
            && !self.resubmitted_requests.contains_key(&req_id)
        {
            self.waiting_for_connection.entry(peer_id).or_default().insert(req_id);
            return if !self.lookup_timeouts.contains(peer_id) {
                log::debug!("Requesting lookup for peer {peer_id}");
                _ = self.lookup_timeouts.try_push(peer_id, futures::future::pending());
                Some(ToSwarm::GenerateEvent(ClientEvent::PeerUnknown { peer_id }))
            } else {
                None
            };
        }

        // Remove the request from buffer – it will not be re-submitted
        self.original_requests.remove(&req_id);

        // Retrieve original request ID, if it was resubmitted
        req_id = self.resubmitted_requests.remove(&req_id).unwrap_or(req_id);

        let ev = match error {
            OutboundFailure::Timeout => ClientEvent::Timeout {
                peer_id,
                req_id,
                timeout: Timeout::Request,
            },
            e => ClientEvent::Failure {
                peer_id,
                req_id,
                error: e.to_string(),
            },
        };
        Some(ToSwarm::GenerateEvent(ev))
    }
}

impl<C> BehaviourWrapper for ClientBehaviour<C>
where
    C: Codec + Clone + Send + 'static,
    C::Request: Clone,
{
    type Inner = request_response::Behaviour<C>;
    type Event = ClientEvent<C::Response>;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_swarm_event(&mut self, event: FromSwarm) -> impl IntoIterator<Item = TToSwarm<Self>> {
        if let FromSwarm::ConnectionEstablished(ConnectionEstablished { peer_id, .. }) = event {
            self.on_connection_established(peer_id)
        }
        None
    }

    fn on_inner_event(
        &mut self,
        ev: request_response::Event<C::Request, C::Response>,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        match ev {
            request_response::Event::Message {
                peer,
                message:
                    request_response::Message::Response {
                        request_id,
                        response,
                    },
                ..
            } => self.on_success(peer, request_id, response),
            request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
            } => self.on_failure(peer, request_id, error),
            _ => None,
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<impl IntoIterator<Item = TToSwarm<Self>>> {
        match self.lookup_timeouts.poll_unpin(cx) {
            Poll::Ready((peer_id, Err(_))) => Poll::Ready(self.on_lookup_timeout(peer_id)),
            Poll::Pending => Poll::Pending,
            _ => unreachable!(), // future::pending() should never complete
        }
    }
}
