use std::{
    fmt::Debug,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use async_trait::async_trait;
use derivative::Derivative;
use futures::{AsyncReadExt, AsyncWriteExt};
use libp2p::request_response;

use crate::PeerId;

pub struct LegacyCodec<T: MsgContent> {
    _phantom: PhantomData<T>,
}

impl<T: MsgContent> Default for LegacyCodec<T> {
    fn default() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }
}

impl<T: MsgContent> Clone for LegacyCodec<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: MsgContent> Copy for LegacyCodec<T> {}

#[async_trait]
impl<M: MsgContent> request_response::Codec for LegacyCodec<M> {
    type Protocol = &'static str;
    type Request = M;
    type Response = u8;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Request>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let mut buf = [0u8; 8];
        io.read_exact(&mut buf).await?;
        let msg_len = u64::from_be_bytes(buf);

        let mut buf = Vec::new();
        io.take(100 * 1024 * 1024).read_to_end(&mut buf).await?;
        if buf.len() as u64 != msg_len {
            log::warn!("Received message size mismatch: {} != {}", buf.len(), msg_len);
        }
        Ok(M::from_vec(buf))
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
        io.take(100).read_to_end(&mut buf).await?;
        Ok(0)
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
        let req = req.as_slice();
        let msg_len = (req.len() as u64).to_be_bytes();
        io.write_all(&msg_len).await?;
        io.write_all(req).await
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
        io.write_all(&[res]).await
    }
}

pub trait MsgContent: Sized + Send + Debug + 'static {
    fn new(size: usize) -> Self;
    fn as_slice(&self) -> &[u8];
    fn as_mut_slice(&mut self) -> &mut [u8];
    fn to_vec(self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
    fn from_vec(vec: Vec<u8>) -> Self {
        let mut content = Self::new(vec.len());
        content.as_mut_slice().copy_from_slice(vec.as_slice());
        content
    }
}

impl MsgContent for Box<[u8]> {
    fn new(size: usize) -> Self {
        vec![0; size].into_boxed_slice()
    }

    fn as_slice(&self) -> &[u8] {
        self.deref()
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        self.deref_mut()
    }

    fn from_vec(vec: Vec<u8>) -> Self {
        vec.into_boxed_slice()
    }
}

impl MsgContent for Vec<u8> {
    fn new(size: usize) -> Self {
        vec![0; size]
    }

    fn as_slice(&self) -> &[u8] {
        self.as_slice()
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }

    fn to_vec(self) -> Vec<u8> {
        self
    }

    fn from_vec(vec: Vec<u8>) -> Self {
        vec
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct Message<T: MsgContent> {
    // None for outgoing broadcast messages, Some for others
    pub peer_id: Option<PeerId>,
    // None for direct messages, Some for broadcast messages
    pub topic: Option<String>,
    #[derivative(Debug = "ignore")]
    pub content: T,
}
