use crate::behaviour::wrapped::{BehaviourWrapper, TToSwarm};
use derivative::Derivative;
use libp2p::{
    request_response,
    request_response::{Codec, ProtocolSupport, ResponseChannel},
    swarm::ToSwarm,
    PeerId,
};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct Request<Req, Res> {
    pub peer_id: PeerId,
    #[derivative(Debug = "ignore")]
    pub request: Req,
    #[derivative(Debug = "ignore")]
    pub response_channel: ResponseChannel<Res>,
}

pub struct ServerBehaviour<C>
where
    C: Codec + Clone + Send + 'static,
{
    inner: request_response::Behaviour<C>,
}

impl<C> ServerBehaviour<C>
where
    C: Codec + Clone + Send + 'static,
{
    pub fn new(codec: C, protocol: C::Protocol) -> Self {
        let inner = request_response::Behaviour::with_codec(
            codec,
            vec![(protocol, ProtocolSupport::Inbound)],
            request_response::Config::default(),
        );
        Self { inner }
    }

    pub fn try_send_response(
        &mut self,
        resp_chan: ResponseChannel<C::Response>,
        response: C::Response,
    ) -> Result<(), C::Response> {
        self.inner.send_response(resp_chan, response)
    }
}

impl<C> BehaviourWrapper for ServerBehaviour<C>
where
    C: Codec + Clone + Send + 'static,
{
    type Inner = request_response::Behaviour<C>;
    type Event = Request<C::Request, C::Response>;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_inner_event(
        &mut self,
        ev: request_response::Event<C::Request, C::Response>,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        match ev {
            request_response::Event::Message {
                peer,
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
            } => {
                return Some(ToSwarm::GenerateEvent(Request {
                    peer_id: peer,
                    request,
                    response_channel: channel,
                }))
            }
            request_response::Event::InboundFailure { peer, error, .. } => {
                log::error!("Request from {peer} failed: {error:?}")
            }
            _ => {}
        }
        None
    }
}
