use std::{
    any::TypeId,
    io::{Cursor, ErrorKind},
    marker::PhantomData,
};

use async_trait::async_trait;
use futures::{AsyncReadExt, AsyncWriteExt};
use libp2p::request_response;
use prost::{
    bytes::{Buf, BytesMut},
    Message,
};

pub struct ProtoVecCodec<Req, Res> {
    _req: PhantomData<Req>,
    _res: PhantomData<Res>,
    max_req_size: u64,
    max_res_size: u64,
}

impl<Req, Res> ProtoVecCodec<Req, Res> {
    pub fn new(max_req_size: u64, max_res_size: u64) -> Self {
        Self {
            _req: Default::default(),
            _res: Default::default(),
            max_req_size,
            max_res_size,
        }
    }
}

impl<Req, Res> Clone for ProtoVecCodec<Req, Res> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Req, Res> Copy for ProtoVecCodec<Req, Res> {}

#[async_trait]
impl<Req: Message + Default + 'static, Res: Message + Default + 'static> request_response::Codec
    for ProtoVecCodec<Req, Res>
{
    type Protocol = &'static str;
    type Request = Vec<Req>;
    type Response = Res;

    async fn read_request<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Request>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        let bytes_read = io.take(self.max_req_size + 1).read_to_end(&mut buf).await?;
        if bytes_read as u64 > self.max_req_size {
            return Err(std::io::Error::new(ErrorKind::InvalidData, "Request too large to read"));
        }
        // Empty request is fine if the Req type is ()
        if buf.is_empty() && TypeId::of::<Req>() != TypeId::of::<()>() {
            log::warn!("{protocol}: received an empty request");
        }
        // let mut decoding_buffer = Bytes::from(buf);
        let mut decoding_cursor = Cursor::new(buf);
        let mut result = Vec::default();
        let mut is_ok = true;
        while decoding_cursor.has_remaining() {
            match Req::decode_length_delimited(&mut decoding_cursor) {
                Ok(msg) => {
                    result.push(msg);
                }
                Err(err) => {
                    log::debug!("Failed to decode incoming buffer as vector: {err}, let's retry single entity");
                    is_ok = false;
                    break;
                }
            }
        }
        if !is_ok {
            decoding_cursor.set_position(0);
            match Req::decode(&mut decoding_cursor) {
                Ok(msg) => {
                    result.push(msg);
                }
                Err(err) => {
                    log::error!("Failed to decode incoming buffer both as single entity and a vector: {err}");
                    return Err(err.into());
                }
            }
        }
        Ok(result)
    }

    async fn read_response<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Response>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        let bytes_read = io.take(self.max_res_size + 1).read_to_end(&mut buf).await?;
        if bytes_read as u64 > self.max_res_size {
            return Err(std::io::Error::new(ErrorKind::InvalidData, "Response too large to read"));
        }
        // Empty response is fine if the Res type is ()
        if buf.is_empty() && TypeId::of::<Res>() != TypeId::of::<()>() {
            log::warn!("{protocol}: received an empty response");
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
        let estimated_len: usize = req.iter().map(|msg| msg.encoded_len() + 1).sum();
        if estimated_len as u64 > self.max_req_size {
            return Err(std::io::Error::new(ErrorKind::InvalidData, "Request too large to write"));
        }
        let mut buffer = BytesMut::new();
        for msg in req {
            msg.encode_length_delimited(&mut buffer)?;
        }
        if buffer.len() as u64 > self.max_req_size {
            return Err(std::io::Error::new(ErrorKind::InvalidData, "Request too large to write"));
        }
        io.write_all(buffer.as_ref()).await
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
        if res.encoded_len() as u64 > self.max_res_size {
            return Err(std::io::Error::new(ErrorKind::InvalidData, "Response too large to write"));
        }
        let buf = res.encode_to_vec();
        io.write_all(buf.as_slice()).await
    }
}
