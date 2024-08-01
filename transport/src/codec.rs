use std::marker::PhantomData;

use async_trait::async_trait;
use futures::{AsyncReadExt, AsyncWriteExt};
use libp2p::request_response;
use prost::Message;

pub const ACK_SIZE: u64 = 4;

pub struct ProtoCodec<Req, Res> {
    _req: PhantomData<Req>,
    _res: PhantomData<Res>,
    max_req_size: u64,
    max_res_size: u64,
}

impl<Req, Res> ProtoCodec<Req, Res> {
    pub fn new(max_req_size: u64, max_res_size: u64) -> Self {
        Self {
            _req: Default::default(),
            _res: Default::default(),
            max_req_size,
            max_res_size,
        }
    }
}

impl<Req, Res> Clone for ProtoCodec<Req, Res> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Req, Res> Copy for ProtoCodec<Req, Res> {}

#[async_trait]
impl<Req: Message + Default, Res: Message + Default> request_response::Codec
    for ProtoCodec<Req, Res>
{
    type Protocol = &'static str;
    type Request = Req;
    type Response = Res;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Request>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        io.take(self.max_req_size).read_to_end(&mut buf).await?;
        if buf.is_empty() {
            log::warn!("Received an empty request");
        }
        Ok(Req::decode(buf.as_slice())?)
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Response>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        io.take(self.max_res_size).read_to_end(&mut buf).await?;
        if buf.is_empty() {
            log::warn!("Received an empty response");
        }
        Ok(Res::decode(buf.as_slice())?)
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        let buf = req.encode_to_vec();
        io.write_all(buf.as_slice()).await
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        let buf = res.encode_to_vec();
        io.write_all(buf.as_slice()).await
    }
}
